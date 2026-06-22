"""
Clip Recorder — Replay screen recorder.

Capture continue de l'écran + son via FFmpeg.
Ctrl+Alt+R sauvegarde les dernières X secondes en MP4.
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
import wave
import pystray
from PIL import Image, ImageDraw, ImageTk
import pyaudiowpatch as pyaudio

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
FPS_OPTIONS = [30, 60, 120]
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


# ─── WASAPI Audio Capture ────────────────────────────────────────────────────

class AudioCapture:
    """Captures system audio via WASAPI loopback into a circular buffer."""

    def __init__(self, max_seconds=120):
        self.max_seconds = max_seconds
        self._pa = None
        self._stream = None
        self._lock = threading.Lock()
        self._buffer = bytearray()
        self._channels = 2
        self._rate = 48000
        self._sample_width = 2  # 16-bit
        self._running = False
        self._loopback_device = None
        self._detect()

    def _detect(self):
        try:
            self._pa = pyaudio.PyAudio()
            wasapi = self._pa.get_host_api_info_by_type(pyaudio.paWASAPI)
            speakers = self._pa.get_device_info_by_index(wasapi["defaultOutputDevice"])
            self._rate = int(speakers["defaultSampleRate"])
            self._channels = speakers["maxOutputChannels"]

            if speakers.get("isLoopbackDevice"):
                self._loopback_device = speakers
            else:
                for i in range(self._pa.get_device_count()):
                    dev = self._pa.get_device_info_by_index(i)
                    if (dev.get("name", "").startswith(speakers["name"])
                            and dev.get("isLoopbackDevice")):
                        self._loopback_device = dev
                        self._channels = dev["maxInputChannels"]
                        break
        except Exception:
            self._loopback_device = None

    @property
    def available(self):
        return self._loopback_device is not None

    @property
    def device_name(self):
        if self._loopback_device:
            return self._loopback_device["name"]
        return None

    def start(self):
        if not self.available or self._running:
            return
        self._buffer = bytearray()
        self._running = True
        try:
            self._stream = self._pa.open(
                format=pyaudio.paInt16,
                channels=self._channels,
                rate=self._rate,
                input=True,
                input_device_index=self._loopback_device["index"],
                frames_per_buffer=1024,
                stream_callback=self._callback,
            )
        except Exception:
            self._running = False

    def stop(self):
        self._running = False
        if self._stream:
            try:
                self._stream.stop_stream()
                self._stream.close()
            except Exception:
                pass
            self._stream = None

    def _callback(self, in_data, frame_count, time_info, status):
        if not self._running:
            return (None, pyaudio.paComplete)
        max_bytes = self.max_seconds * self._rate * self._channels * self._sample_width
        with self._lock:
            self._buffer.extend(in_data)
            if len(self._buffer) > max_bytes:
                self._buffer = self._buffer[-max_bytes:]
        return (None, pyaudio.paContinue)

    def get_last_seconds(self, seconds):
        """Return raw PCM bytes for the last N seconds."""
        bytes_needed = seconds * self._rate * self._channels * self._sample_width
        with self._lock:
            if len(self._buffer) >= bytes_needed:
                return bytes(self._buffer[-bytes_needed:])
            return bytes(self._buffer)

    def save_wav(self, path, seconds):
        """Save the last N seconds to a WAV file. Returns True if audio was saved."""
        pcm = self.get_last_seconds(seconds)
        if not pcm:
            return False
        with wave.open(path, "wb") as wf:
            wf.setnchannels(self._channels)
            wf.setsampwidth(self._sample_width)
            wf.setframerate(self._rate)
            wf.writeframes(pcm)
        return True

    def cleanup(self):
        self.stop()
        if self._pa:
            try:
                self._pa.terminate()
            except Exception:
                pass


# ─── Config ──────────────────────────────────────────────────────────────────

DEFAULTS = {
    "monitor": 0,
    "fps": 60,
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
    def __init__(self, root, config, monitors, audio_capture):
        self.root = root
        self.config = config
        self.monitors = monitors
        self.audio = audio_capture
        self.proc = None
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
        fps = self.config.get("fps", 60)
        buffer_secs = self.config.get("buffer_seconds", 30)
        segment_wrap = math.ceil(buffer_secs / SEGMENT_DURATION) + 2
        keyframe_interval = fps * SEGMENT_DURATION
        seg_pattern = os.path.join(self.segment_dir, "seg_%03d.ts")

        # Video input
        cmd = [FFMPEG, "-y"]
        cmd += [
            "-f", "gdigrab",
            "-framerate", str(fps),
            "-offset_x", str(mon["x"]),
            "-offset_y", str(mon["y"]),
            "-video_size", f"{mon['w']}x{mon['h']}",
            "-i", "desktop",
        ]

        # Video encoding
        if self.has_nvenc:
            cmd += [
                "-c:v", "h264_nvenc",
                "-preset", "p1", "-tune", "ll",
                "-rc", "constqp", "-qp", "20",
                "-g", str(keyframe_interval),
            ]
        else:
            cmd += [
                "-c:v", "libx264",
                "-preset", "ultrafast", "-tune", "zerolatency",
                "-crf", "18",
                "-g", str(keyframe_interval),
            ]

        # Segment output
        cmd += [
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
        if self.audio:
            self.audio.start()
        self._start_poll()

    def stop(self):
        self._stop_poll()
        if self.audio:
            self.audio.stop()
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

        has_audio = self.audio and self.audio.available
        wav_path = os.path.join(self.segment_dir, f"audio_{concat_id}.wav") if has_audio else None

        if has_audio:
            self.audio.save_wav(wav_path, replay_secs)

        if has_audio and wav_path and os.path.exists(wav_path):
            video_only = os.path.join(self.segment_dir, f"video_{concat_id}.mp4")
            cmd_video = [
                FFMPEG, "-y",
                "-f", "concat", "-safe", "0",
                "-i", concat_file,
                "-ss", str(ss),
                "-t", str(replay_secs),
                "-c", "copy",
                "-movflags", "+faststart",
                video_only,
            ]
            cmd_mux = [
                FFMPEG, "-y",
                "-i", video_only,
                "-i", wav_path,
                "-c:v", "copy",
                "-c:a", "aac", "-b:a", "192k",
                "-shortest",
                "-movflags", "+faststart",
                output_path,
            ]
        else:
            video_only = None
            cmd_video = [
                FFMPEG, "-y",
                "-f", "concat", "-safe", "0",
                "-i", concat_file,
                "-ss", str(ss),
                "-t", str(replay_secs),
                "-c", "copy",
                "-movflags", "+faststart",
                output_path,
            ]
            cmd_mux = None

        def _run():
            try:
                subprocess.run(
                    cmd_video, capture_output=True, timeout=30,
                    creationflags=0x08000000,
                )
                if cmd_mux:
                    subprocess.run(
                        cmd_mux, capture_output=True, timeout=30,
                        creationflags=0x08000000,
                    )
                winsound.PlaySound(
                    "SystemExclamation",
                    winsound.SND_ALIAS | winsound.SND_ASYNC,
                )
            except Exception:
                pass
            finally:
                for tmp in [concat_file, wav_path, video_only]:
                    if tmp:
                        try:
                            os.remove(tmp)
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
        if self.proc and self.proc.poll() is not None:
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


# ─── Notification Banner ────────────────────────────────────────────────────

GWL_EXSTYLE = -20
WS_EX_TRANSPARENT = 0x00000020
WS_EX_TOOLWINDOW = 0x00000080


class NotificationBanner:
    def __init__(self, root, monitors, config):
        self.root = root
        self.monitors = monitors
        self.config = config
        self._win = None
        self._hide_id = None

    def show(self, text="Clip enregistré", duration_ms=3000):
        if self._win and self._win.winfo_exists():
            self._win.destroy()
        if self._hide_id:
            self.root.after_cancel(self._hide_id)

        mon_idx = min(self.config["monitor"], len(self.monitors) - 1)
        mon = self.monitors[mon_idx]

        self._win = tk.Toplevel(self.root)
        self._win.overrideredirect(True)
        self._win.attributes("-topmost", True)
        self._win.configure(bg="#1a1a2e")

        w, h = 220, 36
        x = mon["x"] + mon["w"] - w - 20
        y = mon["y"] + 20
        self._win.geometry(f"{w}x{h}+{x}+{y}")

        tk.Label(
            self._win, text=f"  {text}  ✓", bg="#1a1a2e", fg="#22cc66",
            font=("Segoe UI", 11, "bold"), anchor="center",
        ).pack(fill="both", expand=True)

        self._win.after(100, self._make_click_through)
        self._hide_id = self.root.after(duration_ms, self._hide)

    def _make_click_through(self):
        if not self._win or not self._win.winfo_exists():
            return
        hwnd = user32.GetParent(self._win.winfo_id())
        if not hwnd:
            hwnd = self._win.winfo_id()
        ex = user32.GetWindowLongPtrW(hwnd, GWL_EXSTYLE)
        user32.SetWindowLongPtrW(
            hwnd, GWL_EXSTYLE,
            ex | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW,
        )
        try:
            ctypes.windll.user32.SetWindowDisplayAffinity(hwnd, 0x00000011)
        except Exception:
            pass

    def _hide(self):
        if self._win and self._win.winfo_exists():
            self._win.destroy()
        self._win = None
        self._hide_id = None


# ─── Hotkey (Ctrl+Alt+R only) ───────────────────────────────────────────────

HOTKEY_SAVE = 1


class HotkeyManager:
    def __init__(self, root, on_save):
        self.root = root
        self.on_save = on_save
        self._thread_id = None
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self):
        self._thread_id = ctypes.windll.kernel32.GetCurrentThreadId()
        user32.RegisterHotKey(None, HOTKEY_SAVE, MOD_CTRL_ALT, 0x52)  # R
        msg = wt.MSG()
        while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
            if msg.message == WM_HOTKEY and msg.wParam == HOTKEY_SAVE:
                self.root.after(0, self.on_save)

    def stop(self):
        if self._thread_id:
            ctypes.windll.user32.PostThreadMessageW(self._thread_id, 0x0012, 0, 0)


# ─── Settings ────────────────────────────────────────────────────────────────

class SettingsWindow:
    def __init__(self, root, config, monitors, capture):
        self.root = root
        self.config = config
        self.monitors = monitors
        self.capture = capture
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
        self.win.geometry("420x420")
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
        self.fps_var = tk.StringVar(value=str(self.config.get("fps", 60)))
        om2 = tk.OptionMenu(row, self.fps_var, *[str(f) for f in FPS_OPTIONS])
        om2.config(bg=BG3, fg=FG, activebackground=BG2, activeforeground=FG,
                   highlightthickness=0, font=FONT_S, relief="flat")
        om2["menu"].config(bg=BG3, fg=FG, activebackground=ACCENT, font=FONT_S)
        om2.pack(side="left")

        # Audio (WASAPI loopback — read-only)
        row = tk.Frame(cf, bg=BG2)
        row.pack(fill="x", padx=10, pady=(4, 2))
        tk.Label(row, text="Audio :", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        audio_name = self.capture.audio.device_name if self.capture.audio and self.capture.audio.available else None
        if audio_name:
            tk.Label(row, text=audio_name, bg=BG2, fg="#22cc66", font=FONT_S,
                     anchor="w").pack(side="left", fill="x", expand=True)
        else:
            tk.Label(row, text="Non disponible", bg=BG2, fg=FG2, font=FONT_S,
                     anchor="w").pack(side="left", fill="x", expand=True)

        # Encoder
        row = tk.Frame(cf, bg=BG2)
        row.pack(fill="x", padx=10, pady=(0, 8))
        tk.Label(row, text="Encodeur :", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        enc = "NVENC (GPU)" if self.capture.has_nvenc else "x264 (CPU)"
        tk.Label(row, text=enc, bg=BG2, fg=ACCENT, font=FONT_B).pack(side="left")

        # ── Replay ──
        self._section("Replay")
        rf = tk.Frame(self.win, bg=BG2, bd=1, relief="flat")
        rf.pack(fill="x", padx=15, pady=(0, 10))

        # Buffer
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

        # ── Raccourci ──
        self._section("Raccourci")
        kf = tk.Frame(self.win, bg=BG2, bd=1, relief="flat")
        kf.pack(fill="x", padx=15, pady=(0, 10))
        row = tk.Frame(kf, bg=BG2)
        row.pack(fill="x", padx=10, pady=6)
        tk.Label(row, text="Ctrl+Alt+R", bg=BG2, fg=ACCENT, font=FONT_B,
                 width=14, anchor="w").pack(side="left")
        tk.Label(row, text="Sauver le clip", bg=BG2, fg=FG2, font=FONT_S).pack(side="left")

        # ── Boutons ──
        bf = tk.Frame(self.win, bg=BG)
        bf.pack(fill="x", padx=15, pady=(8, 10))
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

    def _apply(self):
        mon_str = self.monitor_var.get()
        try:
            monitor = int(mon_str.split(":")[0]) - 1
        except Exception:
            monitor = 0

        new = {
            "monitor": monitor,
            "fps": int(self.fps_var.get()),
            "buffer_seconds": int(self.buffer_var.get()),
            "output_folder": self.folder_var.get(),
        }

        capture_changed = (
            new["monitor"] != self.config["monitor"]
            or new["fps"] != self.config.get("fps")
            or new["buffer_seconds"] != self.config.get("buffer_seconds")
        )
        self.config.update(new)
        if capture_changed:
            self.capture.restart()

    def _save(self):
        self._apply()
        save_config(self.config)

    def _close(self):
        self.win.destroy()
        self.win = None


# ─── System Tray ─────────────────────────────────────────────────────────────

class TrayIcon:
    def __init__(self, root, config, capture, settings, shutdown_fn, save_fn):
        self.root = root
        self.config = config
        self.capture = capture
        self.settings = settings
        self.shutdown_fn = shutdown_fn
        self.save_fn = save_fn
        self.icon = None
        self._start()

    def _start(self):
        image = create_tray_icon_image()
        menu = pystray.Menu(
            pystray.MenuItem(
                "Sauver le clip (Ctrl+Alt+R)",
                lambda: self.root.after(0, self.save_fn),
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
        self.icon = pystray.Icon("Clip Recorder", image, "Clip Recorder — REC", menu)
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

    audio = AudioCapture(max_seconds=max(BUFFER_OPTIONS))
    capture = FFmpegCapture(root, config, monitors, audio)
    banner = NotificationBanner(root, monitors, config)
    settings = SettingsWindow(root, config, monitors, capture)

    tray = None
    hotkeys = None

    def do_save():
        capture.save_replay()
        banner.show()

    def shutdown():
        nonlocal tray, hotkeys
        capture.cleanup()
        audio.cleanup()
        if hotkeys:
            hotkeys.stop()
        if tray:
            tray.stop()
        root.destroy()
        sys.exit(0)

    hotkeys = HotkeyManager(root, do_save)
    tray = TrayIcon(root, config, capture, settings, shutdown, do_save)

    capture.start()
    root.mainloop()


if __name__ == "__main__":
    main()
