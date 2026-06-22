"""
Clip Recorder — Replay screen recorder.

Capture continue de l'écran via FFmpeg. Un raccourci sauvegarde
les dernières X secondes en MP4.

Raccourcis :
  Ctrl+Alt+R → Sauver le replay
  Ctrl+Alt+P → Pause / reprendre
  Ctrl+Alt+S → Paramètres
  Ctrl+Alt+Q → Quitter
"""

import atexit
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import tkinter as tk
from tkinter import filedialog
import uuid
import winsound
import ctypes
import ctypes.wintypes as wt
import pystray
from PIL import Image, ImageDraw, ImageTk

# ─── Chemins ─────────────────────────────────────────────────────────────────

if getattr(sys, "frozen", False):
    SCRIPT_DIR = os.path.dirname(sys.executable)
    BUNDLE_DIR = sys._MEIPASS
else:
    SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
    BUNDLE_DIR = SCRIPT_DIR

CONFIG_FILE = os.path.join(SCRIPT_DIR, "config.json")

_ffmpeg_bundle = os.path.join(BUNDLE_DIR, "ffmpeg.exe")
FFMPEG = _ffmpeg_bundle if os.path.exists(_ffmpeg_bundle) else "ffmpeg"

# ─── Win32 ───────────────────────────────────────────────────────────────────

user32 = ctypes.windll.user32
try:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
except Exception:
    try:
        user32.SetProcessDPIAware()
    except Exception:
        pass

GWL_EXSTYLE = -20
WS_EX_TRANSPARENT = 0x00000020
WM_HOTKEY = 0x0312
MOD_CTRL_ALT = 0x0002 | 0x0001 | 0x4000

# ─── Thème ───────────────────────────────────────────────────────────────────

BG = "#1e1e1e"
BG2 = "#2d2d2d"
BG3 = "#3c3c3c"
FG = "#e0e0e0"
FG2 = "#999999"
ACCENT = "#ff4444"
FONT = ("Segoe UI", 10)
FONT_B = ("Segoe UI", 10, "bold")
FONT_S = ("Segoe UI", 9)

# ─── Constantes capture ─────────────────────────────────────────────────────

SEGMENT_DURATION = 5
QUALITY_LABELS = ["Basse (rapide)", "Moyenne", "Haute (lent)"]
QUALITY_CRF = [32, 23, 18]
QUALITY_QP = [32, 28, 23]
FPS_OPTIONS = [15, 30, 60]
BUFFER_OPTIONS = [15, 30, 60, 90, 120]

# ─── Moniteurs ───────────────────────────────────────────────────────────────


class MONITORINFOEXW(ctypes.Structure):
    _fields_ = [
        ("cbSize", wt.DWORD),
        ("rcMonitor", wt.RECT),
        ("rcWork", wt.RECT),
        ("dwFlags", wt.DWORD),
        ("szDevice", wt.WCHAR * 32),
    ]


def get_monitors():
    monitors = []
    MONITORENUMPROC = ctypes.WINFUNCTYPE(
        wt.BOOL, wt.HANDLE, wt.HDC, ctypes.POINTER(wt.RECT), wt.LPARAM
    )

    def callback(hMonitor, hdcMonitor, lprcMonitor, dwData):
        info = MONITORINFOEXW()
        info.cbSize = ctypes.sizeof(MONITORINFOEXW)
        user32.GetMonitorInfoW(hMonitor, ctypes.byref(info))
        rc = info.rcMonitor
        monitors.append({
            "name": info.szDevice.strip("\x00"),
            "x": rc.left, "y": rc.top,
            "w": rc.right - rc.left, "h": rc.bottom - rc.top,
            "primary": bool(info.dwFlags & 1),
        })
        return True

    cb_ref = MONITORENUMPROC(callback)
    user32.EnumDisplayMonitors(None, None, cb_ref, 0)
    monitors.sort(key=lambda m: (not m["primary"], m["x"], m["y"]))
    return monitors


# ─── Config ──────────────────────────────────────────────────────────────────

DEFAULTS = {
    "monitor": 0,
    "fps": 30,
    "quality": 1,
    "buffer_seconds": 30,
    "output_folder": "",
}


def load_config():
    if os.path.exists(CONFIG_FILE):
        try:
            with open(CONFIG_FILE, "r", encoding="utf-8") as f:
                cfg = json.load(f)
            for k, v in DEFAULTS.items():
                cfg.setdefault(k, v)
            return cfg
        except Exception:
            pass
    return dict(DEFAULTS)


def save_config(cfg):
    with open(CONFIG_FILE, "w", encoding="utf-8") as f:
        json.dump(cfg, f, indent=2, ensure_ascii=False)


def get_output_folder(cfg):
    folder = cfg.get("output_folder") or ""
    if not folder:
        folder = os.path.join(SCRIPT_DIR, "Clips")
    return folder


# ─── Tray icon image ────────────────────────────────────────────────────────

def create_tray_icon_image(size=64):
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    mid = size // 2
    r = size // 3
    draw.ellipse([mid - r, mid - r, mid + r, mid + r], fill=(255, 68, 68, 255))
    return img


# ─── NVENC detection ─────────────────────────────────────────────────────────

def detect_nvenc():
    try:
        result = subprocess.run(
            [FFMPEG, "-encoders"],
            capture_output=True, text=True, timeout=5,
            creationflags=0x08000000,
        )
        return "h264_nvenc" in result.stdout
    except Exception:
        return False


# ─── FFmpeg Capture ──────────────────────────────────────────────────────────

class FFmpegCapture:
    def __init__(self, root, config, monitors, on_status_change=None):
        self.root = root
        self.config = config
        self.monitors = monitors
        self.on_status_change = on_status_change
        self.proc = None
        self.paused = False
        self.segment_dir = tempfile.mkdtemp(prefix="cliprec_")
        self.has_nvenc = detect_nvenc()
        self._poll_id = None
        atexit.register(self.cleanup)

    def start(self):
        if self.proc and self.proc.poll() is None:
            return

        os.makedirs(self.segment_dir, exist_ok=True)

        mon_idx = min(self.config["monitor"], len(self.monitors) - 1)
        mon = self.monitors[mon_idx]
        fps = self.config.get("fps", 30)
        quality = self.config.get("quality", 1)
        buffer_secs = self.config.get("buffer_seconds", 30)
        segment_wrap = math.ceil(buffer_secs / SEGMENT_DURATION) + 2
        seg_pattern = os.path.join(self.segment_dir, "seg_%03d.ts")

        if self.has_nvenc:
            cmd = [
                FFMPEG, "-y",
                "-f", "gdigrab",
                "-framerate", str(fps),
                "-offset_x", str(mon["x"]),
                "-offset_y", str(mon["y"]),
                "-video_size", f"{mon['w']}x{mon['h']}",
                "-i", "desktop",
                "-c:v", "h264_nvenc",
                "-preset", "p1", "-tune", "ll",
                "-rc", "constqp", "-qp", str(QUALITY_QP[quality]),
                "-f", "segment",
                "-segment_time", str(SEGMENT_DURATION),
                "-segment_wrap", str(segment_wrap),
                "-reset_timestamps", "1",
                "-segment_format", "mpegts",
                seg_pattern,
            ]
        else:
            cmd = [
                FFMPEG, "-y",
                "-f", "gdigrab",
                "-framerate", str(fps),
                "-offset_x", str(mon["x"]),
                "-offset_y", str(mon["y"]),
                "-video_size", f"{mon['w']}x{mon['h']}",
                "-i", "desktop",
                "-c:v", "libx264",
                "-preset", "ultrafast", "-tune", "zerolatency",
                "-crf", str(QUALITY_CRF[quality]),
                "-f", "segment",
                "-segment_time", str(SEGMENT_DURATION),
                "-segment_wrap", str(segment_wrap),
                "-reset_timestamps", "1",
                "-segment_format", "mpegts",
                seg_pattern,
            ]

        self.proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=0x08000000,
        )
        self.paused = False
        self._start_poll()
        if self.on_status_change:
            self.on_status_change("recording")

    def stop(self):
        self._stop_poll()
        if self.proc and self.proc.poll() is None:
            try:
                self.proc.stdin.write(b"q")
                self.proc.stdin.flush()
            except Exception:
                pass
            try:
                self.proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        self.proc = None

    def toggle_pause(self):
        if self.paused:
            self.start()
        else:
            self.stop()
            self.paused = True
            if self.on_status_change:
                self.on_status_change("paused")

    def restart(self):
        self.stop()
        for f in os.listdir(self.segment_dir):
            try:
                os.remove(os.path.join(self.segment_dir, f))
            except Exception:
                pass
        self.start()

    def save_replay(self):
        replay_secs = self.config.get("buffer_seconds", 30)
        output_folder = get_output_folder(self.config)
        os.makedirs(output_folder, exist_ok=True)

        try:
            files = [
                f for f in os.listdir(self.segment_dir)
                if f.startswith("seg_") and f.endswith(".ts")
            ]
        except Exception:
            return

        if not files:
            return

        files_with_mtime = []
        for f in files:
            path = os.path.join(self.segment_dir, f)
            try:
                files_with_mtime.append((f, os.path.getmtime(path)))
            except Exception:
                pass

        files_with_mtime.sort(key=lambda x: x[1])

        if len(files_with_mtime) > 1:
            files_with_mtime = files_with_mtime[:-1]

        segments_needed = math.ceil(replay_secs / SEGMENT_DURATION) + 1
        selected = files_with_mtime[-segments_needed:]

        if not selected:
            return

        concat_id = uuid.uuid4().hex[:8]
        concat_file = os.path.join(self.segment_dir, f"concat_{concat_id}.txt")
        with open(concat_file, "w", encoding="utf-8") as fh:
            for seg_name, _ in selected:
                seg_path = os.path.join(self.segment_dir, seg_name).replace("\\", "/")
                fh.write(f"file '{seg_path}'\n")

        total_duration = len(selected) * SEGMENT_DURATION
        ss = max(0, total_duration - replay_secs)

        timestamp = time.strftime("%Y%m%d_%H%M%S")
        output_path = os.path.join(output_folder, f"Clip_{timestamp}.mp4")

        cmd = [
            FFMPEG, "-y",
            "-f", "concat", "-safe", "0",
            "-i", concat_file,
            "-ss", str(ss),
            "-t", str(replay_secs),
            "-c", "copy",
            "-movflags", "+faststart",
            output_path,
        ]

        def _run():
            try:
                subprocess.run(
                    cmd, capture_output=True, timeout=30,
                    creationflags=0x08000000,
                )
                winsound.PlaySound(
                    "SystemExclamation",
                    winsound.SND_ALIAS | winsound.SND_ASYNC,
                )
            except Exception:
                pass
            finally:
                try:
                    os.remove(concat_file)
                except Exception:
                    pass

        threading.Thread(target=_run, daemon=True).start()

    def _start_poll(self):
        self._poll_id = self.root.after(2000, self._check_health)

    def _stop_poll(self):
        if self._poll_id:
            self.root.after_cancel(self._poll_id)
            self._poll_id = None

    def _check_health(self):
        if self.proc and self.proc.poll() is not None and not self.paused:
            self.proc = None
            self.root.after(1000, self.start)
            return
        self._poll_id = self.root.after(2000, self._check_health)

    def cleanup(self):
        self.stop()
        try:
            shutil.rmtree(self.segment_dir, ignore_errors=True)
        except Exception:
            pass


# ─── Hotkeys ─────────────────────────────────────────────────────────────────

HOTKEY_SAVE = 1
HOTKEY_PAUSE = 2
HOTKEY_SETTINGS = 3
HOTKEY_QUIT = 4

HOTKEY_DEFS = {
    HOTKEY_SAVE:     0x52,  # R
    HOTKEY_PAUSE:    0x50,  # P
    HOTKEY_SETTINGS: 0x53,  # S
    HOTKEY_QUIT:     0x51,  # Q
}


class HotkeyManager:
    def __init__(self, root, callbacks):
        self.root = root
        self.callbacks = callbacks
        self._thread_id = None
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self):
        self._thread_id = ctypes.windll.kernel32.GetCurrentThreadId()
        for hk_id, vk in HOTKEY_DEFS.items():
            user32.RegisterHotKey(None, hk_id, MOD_CTRL_ALT, vk)
        msg = wt.MSG()
        while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
            if msg.message == WM_HOTKEY:
                hk_id = msg.wParam
                cb = self.callbacks.get(hk_id)
                if cb:
                    self.root.after(0, cb)

    def stop(self):
        if self._thread_id:
            ctypes.windll.user32.PostThreadMessageW(self._thread_id, 0x0012, 0, 0)


# ─── Status Overlay ──────────────────────────────────────────────────────────

class StatusOverlay:
    def __init__(self, root, monitors, config):
        self.root = root
        self.monitors = monitors
        self.config = config
        self.status = "recording"
        self.TC = "#FF00FE"

        self.win = tk.Toplevel(root)
        self.win.withdraw()
        self.win.title("ClipRecStatus")
        self.win.overrideredirect(True)
        self.win.attributes("-topmost", True)
        self.win.attributes("-transparentcolor", self.TC)
        self.win.config(bg=self.TC)

        self.canvas = tk.Canvas(
            self.win, width=90, height=28,
            bg=self.TC, highlightthickness=0, bd=0,
        )
        self.canvas.pack()

        self._position()
        self._draw()
        self.win.deiconify()
        self.win.after(500, self._make_click_through)
        self._keep_on_top()

    def _position(self):
        mon_idx = min(self.config["monitor"], len(self.monitors) - 1)
        mon = self.monitors[mon_idx]
        x = mon["x"] + mon["w"] - 110
        y = mon["y"] + 16
        self.win.geometry(f"90x28+{x}+{y}")

    def _draw(self):
        self.canvas.delete("all")
        if self.status == "recording":
            self.canvas.create_oval(6, 7, 20, 21, fill="#ff4444", outline="")
            self.canvas.create_text(52, 14, text="REC", fill="#ffffff",
                                    font=("Segoe UI", 9, "bold"))
        elif self.status == "paused":
            self.canvas.create_text(45, 14, text="PAUSE", fill="#ffaa00",
                                    font=("Segoe UI", 9, "bold"))

    def set_status(self, status):
        self.status = status
        self._draw()

    def _make_click_through(self):
        hwnd = user32.GetParent(self.win.winfo_id())
        if not hwnd:
            hwnd = self.win.winfo_id()
        self.hwnd = hwnd
        ex = user32.GetWindowLongPtrW(hwnd, GWL_EXSTYLE)
        user32.SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_TRANSPARENT)
        try:
            ctypes.windll.user32.SetWindowDisplayAffinity(hwnd, 0x00000011)
        except Exception:
            pass

    def _keep_on_top(self):
        self.win.attributes("-topmost", True)
        self.root.after(2000, self._keep_on_top)


# ─── Settings ────────────────────────────────────────────────────────────────

class SettingsWindow:
    def __init__(self, root, config, monitors, capture, overlay):
        self.root = root
        self.config = config
        self.monitors = monitors
        self.capture = capture
        self.overlay = overlay
        self.win = None

    def toggle(self):
        if self.win and self.win.winfo_exists():
            self.win.destroy()
            self.win = None
            return
        self._build()

    def _build(self):
        self.win = tk.Toplevel(self.root)
        self.win.title("Clip Recorder — Paramètres")
        self.win.geometry("400x500")
        self.win.resizable(False, False)
        self.win.configure(bg=BG)
        self.win.attributes("-topmost", True)
        self._icon_photo = ImageTk.PhotoImage(create_tray_icon_image(32))
        self.win.iconphoto(False, self._icon_photo)
        self.win.protocol("WM_DELETE_WINDOW", self._close)

        # ── Capture ──
        self._section("Capture")
        cf = tk.Frame(self.win, bg=BG2, bd=1, relief="flat")
        cf.pack(fill="x", padx=15, pady=(0, 10))

        # Monitor
        row = tk.Frame(cf, bg=BG2)
        row.pack(fill="x", padx=10, pady=(8, 4))
        tk.Label(row, text="Écran :", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        mon_labels = []
        for i, m in enumerate(self.monitors):
            tag = " ★" if m["primary"] else ""
            mon_labels.append(f"{i+1}: {m['name']} ({m['w']}×{m['h']}){tag}")
        self.monitor_var = tk.StringVar(
            value=mon_labels[min(self.config["monitor"], len(mon_labels) - 1)]
        )
        om = tk.OptionMenu(row, self.monitor_var, *mon_labels)
        om.config(bg=BG3, fg=FG, activebackground=BG2, activeforeground=FG,
                  highlightthickness=0, font=FONT_S, relief="flat")
        om["menu"].config(bg=BG3, fg=FG, activebackground=ACCENT, font=FONT_S)
        om.pack(side="left", fill="x", expand=True)

        # FPS
        row = tk.Frame(cf, bg=BG2)
        row.pack(fill="x", padx=10, pady=4)
        tk.Label(row, text="FPS :", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        self.fps_var = tk.StringVar(value=str(self.config.get("fps", 30)))
        om2 = tk.OptionMenu(row, self.fps_var, *[str(f) for f in FPS_OPTIONS])
        om2.config(bg=BG3, fg=FG, activebackground=BG2, activeforeground=FG,
                   highlightthickness=0, font=FONT_S, relief="flat")
        om2["menu"].config(bg=BG3, fg=FG, activebackground=ACCENT, font=FONT_S)
        om2.pack(side="left")

        # Quality
        row = tk.Frame(cf, bg=BG2)
        row.pack(fill="x", padx=10, pady=4)
        tk.Label(row, text="Qualité :", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        self.quality_var = tk.StringVar(
            value=QUALITY_LABELS[self.config.get("quality", 1)]
        )
        om3 = tk.OptionMenu(row, self.quality_var, *QUALITY_LABELS)
        om3.config(bg=BG3, fg=FG, activebackground=BG2, activeforeground=FG,
                   highlightthickness=0, font=FONT_S, relief="flat")
        om3["menu"].config(bg=BG3, fg=FG, activebackground=ACCENT, font=FONT_S)
        om3.pack(side="left", fill="x", expand=True)

        # Encoder (read-only)
        row = tk.Frame(cf, bg=BG2)
        row.pack(fill="x", padx=10, pady=(4, 8))
        tk.Label(row, text="Encodeur :", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        enc = "NVENC (GPU)" if self.capture.has_nvenc else "x264 (CPU)"
        tk.Label(row, text=enc, bg=BG2, fg=ACCENT, font=FONT_B).pack(side="left")

        # ── Replay ──
        self._section("Replay")
        rf = tk.Frame(self.win, bg=BG2, bd=1, relief="flat")
        rf.pack(fill="x", padx=15, pady=(0, 10))

        # Buffer duration
        row = tk.Frame(rf, bg=BG2)
        row.pack(fill="x", padx=10, pady=(8, 4))
        tk.Label(row, text="Durée (s) :", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        self.buffer_var = tk.StringVar(
            value=str(self.config.get("buffer_seconds", 30))
        )
        om4 = tk.OptionMenu(row, self.buffer_var, *[str(b) for b in BUFFER_OPTIONS])
        om4.config(bg=BG3, fg=FG, activebackground=BG2, activeforeground=FG,
                   highlightthickness=0, font=FONT_S, relief="flat")
        om4["menu"].config(bg=BG3, fg=FG, activebackground=ACCENT, font=FONT_S)
        om4.pack(side="left")

        # Output folder
        row = tk.Frame(rf, bg=BG2)
        row.pack(fill="x", padx=10, pady=(4, 8))
        tk.Label(row, text="Dossier :", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        self.folder_var = tk.StringVar(value=get_output_folder(self.config))
        tk.Entry(row, textvariable=self.folder_var, bg=BG3, fg=FG,
                 insertbackground=FG, font=FONT_S, relief="flat", bd=2,
                 ).pack(side="left", fill="x", expand=True, padx=(0, 5))
        tk.Button(row, text="...", command=self._browse_folder,
                  bg=BG3, fg=FG, relief="flat", font=FONT_S,
                  cursor="hand2", width=3).pack(side="left")

        # ── Raccourcis ──
        self._section("Raccourcis")
        kf = tk.Frame(self.win, bg=BG2, bd=1, relief="flat")
        kf.pack(fill="x", padx=15, pady=(0, 10))

        shortcuts = [
            ("Ctrl+Alt+R", "Sauver le replay"),
            ("Ctrl+Alt+P", "Pause / reprendre"),
            ("Ctrl+Alt+S", "Paramètres"),
            ("Ctrl+Alt+Q", "Quitter"),
        ]
        for key, desc in shortcuts:
            row = tk.Frame(kf, bg=BG2)
            row.pack(fill="x", padx=10, pady=2)
            tk.Label(row, text=key, bg=BG2, fg=ACCENT, font=FONT_B,
                     width=14, anchor="w").pack(side="left")
            tk.Label(row, text=desc, bg=BG2, fg=FG2, font=FONT_S).pack(side="left")
        tk.Frame(kf, bg=BG2, height=4).pack()

        # ── Boutons ──
        bf = tk.Frame(self.win, bg=BG)
        bf.pack(fill="x", padx=15, pady=(5, 10))
        tk.Button(bf, text="Appliquer", command=self._apply,
                  bg=ACCENT, fg="#ffffff", font=FONT_B, relief="flat",
                  padx=15, cursor="hand2").pack(side="left", padx=(0, 8))
        tk.Button(bf, text="Sauvegarder", command=self._save,
                  bg=BG3, fg=FG, font=FONT, relief="flat",
                  padx=10, cursor="hand2").pack(side="left")

        self.win.lift()
        self.win.focus_force()

    def _section(self, text):
        tk.Label(self.win, text=text, bg=BG, fg=ACCENT, font=FONT_B,
                 anchor="w").pack(fill="x", padx=15, pady=(8, 2))

    def _browse_folder(self):
        folder = filedialog.askdirectory(
            title="Dossier de sortie",
            initialdir=self.folder_var.get(),
        )
        if folder:
            self.folder_var.set(folder)

    def _read_values(self):
        mon_str = self.monitor_var.get()
        try:
            monitor = int(mon_str.split(":")[0]) - 1
        except Exception:
            monitor = 0

        quality = 1
        qval = self.quality_var.get()
        for i, label in enumerate(QUALITY_LABELS):
            if label == qval:
                quality = i
                break

        return {
            "monitor": monitor,
            "fps": int(self.fps_var.get()),
            "quality": quality,
            "buffer_seconds": int(self.buffer_var.get()),
            "output_folder": self.folder_var.get(),
        }

    def _apply(self):
        new = self._read_values()
        capture_changed = (
            new["monitor"] != self.config["monitor"]
            or new["fps"] != self.config.get("fps")
            or new["quality"] != self.config.get("quality")
            or new["buffer_seconds"] != self.config.get("buffer_seconds")
        )
        self.config.update(new)
        if capture_changed:
            self.capture.restart()
            self.overlay._position()

    def _save(self):
        self._apply()
        save_config(self.config)

    def _close(self):
        self.win.destroy()
        self.win = None


# ─── System Tray ─────────────────────────────────────────────────────────────

class TrayIcon:
    def __init__(self, root, config, capture, settings, overlay, shutdown_fn):
        self.root = root
        self.config = config
        self.capture = capture
        self.settings = settings
        self.overlay = overlay
        self.shutdown_fn = shutdown_fn
        self.icon = None
        self._start()

    def _start(self):
        image = create_tray_icon_image()
        menu = pystray.Menu(
            pystray.MenuItem(
                "Sauver le clip",
                lambda: self.root.after(0, self.capture.save_replay),
            ),
            pystray.MenuItem(
                "Pause / Reprendre",
                lambda: self.root.after(0, self.capture.toggle_pause),
            ),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem(
                "Paramètres",
                lambda: self.root.after(0, self.settings.toggle),
            ),
            pystray.MenuItem(
                "Ouvrir le dossier",
                lambda: self.root.after(0, self._open_folder),
            ),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem(
                "Quitter",
                lambda: self.root.after(0, self.shutdown_fn),
            ),
        )
        self.icon = pystray.Icon("Clip Recorder", image, "Clip Recorder", menu)
        threading.Thread(target=self.icon.run, daemon=True).start()

    def _open_folder(self):
        folder = get_output_folder(self.config)
        os.makedirs(folder, exist_ok=True)
        os.startfile(folder)

    def stop(self):
        if self.icon:
            self.icon.stop()


# ─── Main ────────────────────────────────────────────────────────────────────

def main():
    # Check FFmpeg
    try:
        subprocess.run(
            [FFMPEG, "-version"],
            capture_output=True, timeout=5,
            creationflags=0x08000000,
        )
    except Exception:
        import tkinter.messagebox
        r = tk.Tk()
        r.withdraw()
        tkinter.messagebox.showerror(
            "Clip Recorder",
            "ffmpeg.exe introuvable.\n\n"
            "Placez ffmpeg.exe à côté de l'application\n"
            "ou installez FFmpeg dans le PATH.",
        )
        sys.exit(1)

    monitors = get_monitors()
    if not monitors:
        monitors = [{
            "name": "Default", "x": 0, "y": 0,
            "w": user32.GetSystemMetrics(0),
            "h": user32.GetSystemMetrics(1),
            "primary": True,
        }]

    config = load_config()
    config["monitor"] = min(config["monitor"], len(monitors) - 1)

    root = tk.Tk()
    root.withdraw()

    overlay = StatusOverlay(root, monitors, config)

    def on_status_change(status):
        overlay.set_status(status)

    capture = FFmpegCapture(root, config, monitors, on_status_change)
    settings = SettingsWindow(root, config, monitors, capture, overlay)

    tray = None
    hotkeys = None

    def shutdown():
        nonlocal tray, hotkeys
        capture.cleanup()
        if hotkeys:
            hotkeys.stop()
        if tray:
            tray.stop()
        root.destroy()
        sys.exit(0)

    callbacks = {
        HOTKEY_SAVE:     capture.save_replay,
        HOTKEY_PAUSE:    capture.toggle_pause,
        HOTKEY_SETTINGS: settings.toggle,
        HOTKEY_QUIT:     shutdown,
    }

    hotkeys = HotkeyManager(root, callbacks)
    tray = TrayIcon(root, config, capture, settings, overlay, shutdown)

    capture.start()
    root.mainloop()


if __name__ == "__main__":
    main()
