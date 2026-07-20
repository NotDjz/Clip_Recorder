"""
Clip Recorder — Replay screen recorder.

Continuous screen + audio capture via FFmpeg.
Ctrl+Alt+R saves the last X seconds as MP4.
"""

import atexit
import collections
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import traceback
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

# ─── Paths ───────────────────────────────────────────────────────────────────

if getattr(sys, "frozen", False):
    SCRIPT_DIR = os.path.dirname(sys.executable)
    BUNDLE_DIR = sys._MEIPASS
else:
    SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
    BUNDLE_DIR = SCRIPT_DIR

CONFIG_FILE = os.path.join(SCRIPT_DIR, "config.json")
LOG_FILE = os.path.join(SCRIPT_DIR, "clip_recorder.log")

_ffmpeg_bundle = os.path.join(BUNDLE_DIR, "ffmpeg.exe")
FFMPEG = _ffmpeg_bundle if os.path.exists(_ffmpeg_bundle) else "ffmpeg"

_log_lock = threading.Lock()


def log(msg):
    line = f"[{time.strftime('%Y-%m-%d %H:%M:%S')}] {msg}"
    try:
        with _log_lock:
            with open(LOG_FILE, "a", encoding="utf-8") as f:
                f.write(line + "\n")
    except Exception:
        pass

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
MOD_ALT = 0x0001
MOD_CONTROL = 0x0002
MOD_SHIFT = 0x0004
MOD_NOREPEAT = 0x4000

MODIFIER_MAP = {"Ctrl": MOD_CONTROL, "Alt": MOD_ALT, "Shift": MOD_SHIFT}

VK_MAP = {**{chr(c): c for c in range(ord('A'), ord('Z') + 1)},
          **{str(i): 0x30 + i for i in range(10)},
          **{f"F{i}": 0x6F + i for i in range(1, 13)}}

KEYSYM_TO_KEY = {**{chr(c): chr(c).upper() for c in range(ord('a'), ord('z') + 1)},
                 **{chr(c): chr(c) for c in range(ord('A'), ord('Z') + 1)},
                 **{str(i): str(i) for i in range(10)},
                 **{f"F{i}": f"F{i}" for i in range(1, 13)}}

# ─── Theme ───────────────────────────────────────────────────────────────────

BG = "#1e1e1e"
BG2 = "#2d2d2d"
BG3 = "#3c3c3c"
FG = "#e0e0e0"
FG2 = "#999999"
ACCENT = "#ff4444"
FONT = ("Segoe UI", 10)
FONT_B = ("Segoe UI", 10, "bold")
FONT_S = ("Segoe UI", 9)

# ─── Capture constants ──────────────────────────────────────────────────────

SEGMENT_DURATION = 5
FPS_OPTIONS = [30, 60, 120, 240]
BUFFER_OPTIONS = [15, 30, 60, 90, 120]

# ─── Monitors ────────────────────────────────────────────────────────────────


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
    """Captures system audio (WASAPI loopback) + microphone into circular buffers."""

    def __init__(self, max_seconds=120, loopback_name="", mic_name=""):
        self.max_seconds = max_seconds
        self._pa = None
        self._loopback_stream = None
        self._mic_stream = None
        self._loopback_lock = threading.Lock()
        self._mic_lock = threading.Lock()
        # Timestamped chunks: deque of (t_arrival_wallclock, pcm_bytes). We
        # reconstruct the real-time audio window on save from these, padding
        # silence for any gap — WASAPI loopback does NOT deliver continuously
        # (it drops/under-delivers during silence), so a flat byte buffer's
        # timeline diverges from real time and desyncs against mic + video.
        self._loopback_chunks = collections.deque()
        self._mic_chunks = collections.deque()
        self._loopback_bytes = 0
        self._mic_bytes = 0
        self._loopback_frames = 0
        self._mic_frames = 0
        # PortAudio status flags OR-accumulated in the callback (overflow/etc).
        # Cheap bitwise-or, no I/O — surfaced in save_replay's log line.
        self._loopback_status_flags = 0
        self._mic_status_flags = 0
        self._heal_gen = 0
        self._channels = 2
        self._rate = 48000
        self._mic_channels = 2
        self._mic_rate = 48000
        self._sample_width = 2
        self._running = False
        self._loopback_device = None
        self._mic_device = None
        self._configured_loopback = loopback_name
        self._configured_mic = mic_name
        self._detect()

    def _detect(self):
        try:
            self._pa = pyaudio.PyAudio()
            wasapi = self._pa.get_host_api_info_by_type(pyaudio.paWASAPI)

            # --- Loopback (system audio) ---
            if self._configured_loopback:
                for i in range(self._pa.get_device_count()):
                    dev = self._pa.get_device_info_by_index(i)
                    if (dev.get("isLoopbackDevice")
                            and self._configured_loopback in dev.get("name", "")):
                        self._loopback_device = dev
                        self._rate = int(dev["defaultSampleRate"])
                        self._channels = dev["maxInputChannels"]
                        break

            if not self._loopback_device:
                speakers = self._pa.get_device_info_by_index(
                    wasapi["defaultOutputDevice"])
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

            # --- Microphone ---
            if self._configured_mic:
                for i in range(self._pa.get_device_count()):
                    dev = self._pa.get_device_info_by_index(i)
                    if (dev["maxInputChannels"] > 0
                            and not dev.get("isLoopbackDevice")
                            and self._configured_mic in dev.get("name", "")):
                        self._mic_device = dev
                        self._mic_rate = int(dev["defaultSampleRate"])
                        self._mic_channels = dev["maxInputChannels"]
                        break

            if not self._mic_device:
                mic_idx = wasapi.get("defaultInputDevice", -1)
                if mic_idx >= 0:
                    mic = self._pa.get_device_info_by_index(mic_idx)
                    if mic["maxInputChannels"] > 0 and not mic.get(
                            "isLoopbackDevice"):
                        self._mic_device = mic
                        self._mic_rate = int(mic["defaultSampleRate"])
                        self._mic_channels = mic["maxInputChannels"]
        except Exception:
            self._loopback_device = None

    @staticmethod
    def list_loopback_devices():
        pa = pyaudio.PyAudio()
        devices = []
        try:
            for i in range(pa.get_device_count()):
                dev = pa.get_device_info_by_index(i)
                if dev.get("isLoopbackDevice") and dev["maxInputChannels"] > 0:
                    devices.append(dev["name"])
        finally:
            pa.terminate()
        return devices

    @staticmethod
    def list_mic_devices():
        pa = pyaudio.PyAudio()
        devices = []
        try:
            wasapi = pa.get_host_api_info_by_type(pyaudio.paWASAPI)
            for i in range(pa.get_device_count()):
                dev = pa.get_device_info_by_index(i)
                if (dev["maxInputChannels"] > 0
                        and not dev.get("isLoopbackDevice")
                        and dev.get("hostApi") == wasapi["index"]):
                    devices.append(dev["name"])
        except Exception:
            pass
        finally:
            pa.terminate()
        return devices

    @property
    def available(self):
        return self._loopback_device is not None

    @property
    def mic_available(self):
        return self._mic_device is not None

    @property
    def device_name(self):
        if self._loopback_device:
            return self._loopback_device["name"]
        return None

    @property
    def mic_name(self):
        if self._mic_device:
            return self._mic_device["name"]
        return None

    def _open_loopback_stream(self):
        if not self._loopback_device:
            return
        try:
            self._loopback_frames = 0
            self._loopback_status_flags = 0
            self._loopback_stream = self._pa.open(
                format=pyaudio.paInt16,
                channels=self._channels,
                rate=self._rate,
                input=True,
                input_device_index=self._loopback_device["index"],
                frames_per_buffer=4096,
                stream_callback=self._loopback_callback,
            )
            log(f"loopback stream opened: device={self._loopback_device.get('name')!r} "
                f"rate={self._rate} channels={self._channels}")
        except Exception as e:
            self._loopback_stream = None
            log(f"loopback stream OPEN FAILED: device={self._loopback_device.get('name')!r}: {e}")

    def _open_mic_stream(self):
        if not self._mic_device:
            return
        try:
            self._mic_frames = 0
            self._mic_status_flags = 0
            self._mic_stream = self._pa.open(
                format=pyaudio.paInt16,
                channels=self._mic_channels,
                rate=self._mic_rate,
                input=True,
                input_device_index=self._mic_device["index"],
                frames_per_buffer=4096,
                stream_callback=self._mic_callback,
            )
            log(f"mic stream opened: device={self._mic_device.get('name')!r} "
                f"rate={self._mic_rate} channels={self._mic_channels}")
        except Exception as e:
            self._mic_stream = None
            log(f"mic stream OPEN FAILED: device={self._mic_device.get('name')!r}: {e}")

    @staticmethod
    def _stream_alive(stream):
        if stream is None:
            return False
        try:
            return bool(stream.is_active())
        except Exception:
            return False

    def _heal_dead_streams(self, gen):
        """Rescue a capture stream that FAILED to open or has STOPPED. Retries
        a failed (re)open on later ticks — WASAPI can throw a transient host
        error (-9999) right after a close, so one retry usually succeeds.

        It deliberately does NOT close a stream that is open and active merely
        because frames==0: WASAPI loopback legitimately delivers nothing during
        silence, and save-time reconstruction pads those gaps — closing a
        healthy-but-silent stream can hit that host error and leave it dead
        (observed). Health is judged by `is_active()`, not frame count. `gen`
        makes a stale heal thread from a superseded start() cycle exit cleanly."""
        max_attempts = 6
        for _ in range(max_attempts):
            time.sleep(1.5)
            if not self._running or gen != self._heal_gen:
                return

            if self._loopback_device is not None and not self._stream_alive(self._loopback_stream):
                log(f"loopback stream not alive (frames={self._loopback_frames}), reopening")
                if self._loopback_stream is not None:
                    try:
                        self._loopback_stream.stop_stream()
                        self._loopback_stream.close()
                    except Exception:
                        pass
                    self._loopback_stream = None
                self._open_loopback_stream()

            if self._mic_device is not None and not self._stream_alive(self._mic_stream):
                log(f"mic stream not alive (frames={self._mic_frames}), reopening")
                if self._mic_stream is not None:
                    try:
                        self._mic_stream.stop_stream()
                        self._mic_stream.close()
                    except Exception:
                        pass
                    self._mic_stream = None
                self._open_mic_stream()

            lb_done = self._loopback_device is None or self._stream_alive(self._loopback_stream)
            mic_done = self._mic_device is None or self._stream_alive(self._mic_stream)
            if lb_done and mic_done:
                return

    def start(self):
        if self._running:
            return
        with self._loopback_lock:
            self._loopback_chunks.clear()
            self._loopback_bytes = 0
        with self._mic_lock:
            self._mic_chunks.clear()
            self._mic_bytes = 0
        self._running = True

        self._open_loopback_stream()
        self._open_mic_stream()

        if not self._loopback_stream and not self._mic_stream:
            self._running = False
            return

        self._heal_gen += 1
        threading.Thread(
            target=self._heal_dead_streams,
            args=(self._heal_gen,),
            daemon=True,
        ).start()

    def stop(self):
        self._running = False
        for stream in (self._loopback_stream, self._mic_stream):
            if stream:
                try:
                    stream.stop_stream()
                    stream.close()
                except Exception:
                    pass
        self._loopback_stream = None
        self._mic_stream = None

    def _loopback_callback(self, in_data, frame_count, time_info, status):
        if not self._running:
            return (None, pyaudio.paComplete)
        t = time.time()
        self._loopback_status_flags |= status  # cheap; no I/O in the realtime callback
        max_bytes = self.max_seconds * self._rate * self._channels * self._sample_width
        with self._loopback_lock:
            self._loopback_chunks.append((t, in_data))
            self._loopback_bytes += len(in_data)
            while self._loopback_bytes > max_bytes and len(self._loopback_chunks) > 1:
                _, old = self._loopback_chunks.popleft()
                self._loopback_bytes -= len(old)
            self._loopback_frames += frame_count
        return (None, pyaudio.paContinue)

    def _mic_callback(self, in_data, frame_count, time_info, status):
        if not self._running:
            return (None, pyaudio.paComplete)
        t = time.time()
        self._mic_status_flags |= status  # cheap; no I/O in the realtime callback
        max_bytes = self.max_seconds * self._mic_rate * self._mic_channels * self._sample_width
        with self._mic_lock:
            self._mic_chunks.append((t, in_data))
            self._mic_bytes += len(in_data)
            while self._mic_bytes > max_bytes and len(self._mic_chunks) > 1:
                _, old = self._mic_chunks.popleft()
                self._mic_bytes -= len(old)
            self._mic_frames += frame_count
        return (None, pyaudio.paContinue)

    @staticmethod
    def _render_window(chunks, rate, channels, sample_width, t_end, duration):
        """Reconstruct exactly `duration` seconds of audio ending at wall-clock
        `t_end`, placing each stored chunk at its real arrival time and leaving
        silence in any gap. This anchors the audio to real time regardless of
        how irregularly the source delivered (WASAPI loopback drops silence),
        so loopback, mic and video all share one timeline. `chunks` is a list
        of (t_arrival, pcm_bytes); each chunk's samples are treated as ending
        at t_arrival and spanning n_frames/rate before it.

        Returns raw PCM bytes of exactly round(duration*rate) frames, or None
        if nothing overlaps the window at all (fully dead stream).
        """
        frame_size = channels * sample_width
        total_frames = int(round(duration * rate))
        if total_frames <= 0:
            return None
        out = bytearray(total_frames * frame_size)  # zero-filled = silence
        t_start = t_end - duration
        wrote_any = False
        for t_arrival, data in chunks:
            n_frames = len(data) // frame_size
            if n_frames <= 0:
                continue
            chunk_start = t_arrival - n_frames / rate
            # Overlap of [chunk_start, t_arrival] with [t_start, t_end]
            ov_start = max(chunk_start, t_start)
            ov_end = min(t_arrival, t_end)
            if ov_end <= ov_start:
                continue
            # Which frames of this chunk fall in the overlap...
            src_from = int(round((ov_start - chunk_start) * rate))
            src_to = int(round((ov_end - chunk_start) * rate))
            src_from = max(0, min(src_from, n_frames))
            src_to = max(0, min(src_to, n_frames))
            if src_to <= src_from:
                continue
            # ...and where they land in the output grid.
            dst_from = int(round((ov_start - t_start) * rate))
            n = src_to - src_from
            if dst_from < 0:
                src_from -= dst_from
                n += dst_from
                dst_from = 0
            if dst_from + n > total_frames:
                n = total_frames - dst_from
            if n <= 0:
                continue
            src_b = src_from * frame_size
            dst_b = dst_from * frame_size
            out[dst_b:dst_b + n * frame_size] = data[src_b:src_b + n * frame_size]
            wrote_any = True
        return bytes(out) if wrote_any else None

    def save_wav(self, path, t_end, duration):
        """Save `duration` seconds of loopback audio ending at wall-clock
        `t_end`, reconstructed on the real timeline (see _render_window)."""
        with self._loopback_lock:
            chunks = list(self._loopback_chunks)
        pcm = self._render_window(chunks, self._rate, self._channels,
                                  self._sample_width, t_end, duration)
        if not pcm:
            return False
        with wave.open(path, "wb") as wf:
            wf.setnchannels(self._channels)
            wf.setsampwidth(self._sample_width)
            wf.setframerate(self._rate)
            wf.writeframes(pcm)
        return True

    def save_mic_wav(self, path, t_end, duration):
        """Save `duration` seconds of microphone audio ending at wall-clock
        `t_end`, reconstructed on the real timeline (see _render_window)."""
        with self._mic_lock:
            chunks = list(self._mic_chunks)
        pcm = self._render_window(chunks, self._mic_rate, self._mic_channels,
                                  self._sample_width, t_end, duration)
        if not pcm:
            return False
        with wave.open(path, "wb") as wf:
            wf.setnchannels(self._mic_channels)
            wf.setsampwidth(self._sample_width)
            wf.setframerate(self._mic_rate)
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
    "loopback_device": "",
    "mic_device": "",
    "hotkey": "Ctrl+Alt+R",
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


class _GUID(ctypes.Structure):
    _fields_ = [
        ("Data1", wt.DWORD),
        ("Data2", wt.WORD),
        ("Data3", wt.WORD),
        ("Data4", ctypes.c_ubyte * 8),
    ]


FOLDERID_VIDEOS = _GUID(
    0x18989B1D, 0x99B5, 0x455B,
    (ctypes.c_ubyte * 8)(0x84, 0x1C, 0xAB, 0x7C, 0x74, 0xE4, 0xDD, 0xFC),
)


def _get_videos_folder():
    try:
        path_ptr = ctypes.c_wchar_p()
        hr = ctypes.windll.shell32.SHGetKnownFolderPath(
            ctypes.byref(FOLDERID_VIDEOS), 0, None, ctypes.byref(path_ptr)
        )
        if hr == 0 and path_ptr.value:
            path = path_ptr.value
            ctypes.windll.ole32.CoTaskMemFree(path_ptr)
            return path
    except Exception:
        pass
    return os.path.join(os.path.expanduser("~"), "Videos")


def get_output_folder(cfg):
    folder = cfg.get("output_folder") or ""
    if not folder:
        folder = os.path.join(_get_videos_folder(), "ClipRecorder")
    return folder


# ─── Tray icon image ────────────────────────────────────────────────────────

def create_tray_icon_image(size=64):
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    mid = size // 2
    r = size // 3
    draw.ellipse([mid - r, mid - r, mid + r, mid + r], fill=(255, 68, 68, 255))
    return img


# ─── FFmpeg feature detection ────────────────────────────────────────────────

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


def detect_ddagrab():
    try:
        result = subprocess.run(
            [FFMPEG, "-filters"],
            capture_output=True, text=True, timeout=5,
            creationflags=0x08000000,
        )
        return "ddagrab" in result.stdout
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
        self.has_ddagrab = detect_ddagrab()
        self._poll_id = None
        atexit.register(self.cleanup)

    def _start_ffmpeg(self):
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

        cmd = [FFMPEG, "-y"]

        if self.has_ddagrab:
            cmd += [
                "-f", "lavfi",
                "-i", f"ddagrab=framerate={fps}:output_idx={mon_idx}:draw_mouse=0",
            ]
        else:
            cmd += [
                "-f", "gdigrab",
                "-framerate", str(fps),
                "-draw_mouse", "0",
                "-offset_x", str(mon["x"]),
                "-offset_y", str(mon["y"]),
                "-video_size", f"{mon['w']}x{mon['h']}",
                "-i", "desktop",
            ]

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

        cmd += [
            "-flush_packets", "1",
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

    def _stop_ffmpeg(self):
        if self.proc and self.proc.poll() is None:
            try:
                self.proc.stdin.write(b"q")
                self.proc.stdin.flush()
            except Exception:
                pass
            try:
                self.proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                try:
                    self.proc.terminate()
                    self.proc.wait(timeout=1)
                except Exception:
                    self.proc.kill()
        self.proc = None

    def _wipe_segments(self):
        # Only the live capture segments (seg_*.ts). Never the transient files a
        # concurrent save_replay._run() owns — snap_*.ts / concat_*.txt / *.wav /
        # video_*.mp4 — which it is actively concatenating/muxing and cleans up
        # itself; deleting those mid-flight would corrupt or fail that save.
        for f in os.listdir(self.segment_dir):
            if not (f.startswith("seg_") and f.endswith(".ts")):
                continue
            try:
                os.remove(os.path.join(self.segment_dir, f))
            except Exception:
                pass

    def start(self):
        if self.proc and self.proc.poll() is None:
            return
        self._start_ffmpeg()
        if self.audio:
            self.audio.start()
        self._start_poll()

    def stop(self):
        self._stop_poll()
        if self.audio:
            self.audio.stop()
        self._stop_ffmpeg()

    def restart_video(self):
        """Re-init ONLY the FFmpeg video process (monitor/fps/buffer_seconds
        change) without touching audio. Audio depends on none of those, and
        restarting it needlessly re-exposes the flaky WASAPI loopback (re)open
        that is the direct trigger of duration-change audio desync. The audio
        circular buffer (max 120s) already spans any buffer_seconds, so it
        just keeps running across the change."""
        self._stop_ffmpeg()
        self._wipe_segments()
        self._start_ffmpeg()

    def save_replay(self, on_success=None):
        save_time = time.time()
        replay_secs = self.config.get("buffer_seconds", 30)
        fps = self.config.get("fps", 60)
        output_folder = get_output_folder(self.config)
        os.makedirs(output_folder, exist_ok=True)

        concat_id = uuid.uuid4().hex[:8]
        log(f"[{concat_id}] save_replay start: fps={fps} buffer_seconds={replay_secs} "
            f"monitor={self.config.get('monitor')}")

        try:
            files = [
                f for f in os.listdir(self.segment_dir)
                if f.startswith("seg_") and f.endswith(".ts")
            ]
        except Exception as e:
            log(f"[{concat_id}] ABORT: listdir failed: {e}")
            return

        if not files:
            log(f"[{concat_id}] ABORT: no segment files found")
            return

        files_with_mtime = []
        for f in files:
            path = os.path.join(self.segment_dir, f)
            try:
                files_with_mtime.append((f, os.path.getmtime(path)))
            except Exception:
                pass

        files_with_mtime.sort(key=lambda x: x[1])

        if not files_with_mtime:
            log(f"[{concat_id}] ABORT: no segment mtimes readable")
            return

        log(f"[{concat_id}] found {len(files_with_mtime)} segments, "
            f"oldest_mtime_age={save_time - files_with_mtime[0][1]:.3f}s "
            f"newest_mtime_age={save_time - files_with_mtime[-1][1]:.3f}s")

        # The newest segment file is still being actively appended to by the
        # live capture process — snapshotting it mid-write can catch a torn,
        # incomplete frame (confirmed via ffmpeg decode: "corrupt decoded
        # frame" / "error while decoding MB"). That shows up as a video
        # glitch/freeze while the audio track — plain PCM, unaffected — keeps
        # playing cleanly, which reads exactly like an A/V desync even though
        # it's actually video corruption. Only ever select from segments that
        # have already been rotated out (fully closed) and are therefore safe
        # to copy.
        complete = files_with_mtime[:-1]
        if not complete:
            log(f"[{concat_id}] ABORT: only the in-progress segment exists yet "
                f"(capture just started/restarted) — nothing complete to save")
            return

        # Select whole segments only, walking backward until their real (mtime-based)
        # combined span covers replay_secs. Every segment boundary is already a real
        # keyframe boundary, so no mid-segment seek is ever needed — this avoids the
        # nominal-vs-real keyframe grid mismatch that a fractional -ss seek runs into
        # once real per-segment duration drifts from the nominal SEGMENT_DURATION
        # (plausible whenever delivered capture rate dips under load, worse at high
        # FPS and over longer buffers since the drift compounds per segment).
        n = len(complete)
        count = 1
        total_duration = None
        while count <= n - 1:
            anchor_mtime = complete[n - count - 1][1]
            candidate_duration = save_time - anchor_mtime
            if candidate_duration >= replay_secs:
                total_duration = candidate_duration
                break
            count += 1
        else:
            count = n
            total_duration = (save_time - complete[0][1]) if n >= 2 else SEGMENT_DURATION

        selected = complete[-count:]

        if not selected:
            log(f"[{concat_id}] ABORT: selection produced empty list")
            return

        log(f"[{concat_id}] selected {len(selected)}/{n} segments, total_duration={total_duration:.3f}s "
            f"(requested {replay_secs}s), avg_seg_duration={total_duration / max(len(selected), 1):.3f}s "
            f"(nominal {SEGMENT_DURATION}s)")

        # Snapshot every selected segment to a uniquely-named copy immediately —
        # the live capture FFmpeg process keeps running and, per -segment_wrap,
        # cyclically overwrites old numbered segment files. The actual concat
        # read only happens later, in the background thread below, after the
        # synchronous audio processing that follows — referencing the original
        # filenames directly would risk one of them being rewritten with fresh
        # content in that gap (a real race, worse with more segments/longer
        # sessions, whose damage when it hits is bounded at exactly one
        # segment's duration). Copying everything up front — the same
        # technique previously used only for the last, in-progress segment —
        # eliminates the race entirely: a live process can never touch a file
        # under a name it never wrote.
        snap_paths = []
        concat_names = []
        snapshot_start = time.time()
        for i, (seg_name, _) in enumerate(selected):
            src = os.path.join(self.segment_dir, seg_name)
            dst_name = f"snap_{concat_id}_{i:03d}.ts"
            dst = os.path.join(self.segment_dir, dst_name)
            try:
                shutil.copy2(src, dst)
            except Exception as e:
                log(f"[{concat_id}] ABORT: snapshot copy failed on segment {i} ({seg_name}): {e}")
                for p in snap_paths:
                    try:
                        os.remove(p)
                    except Exception:
                        pass
                return
            snap_paths.append(dst)
            concat_names.append(dst_name)
        log(f"[{concat_id}] snapshotted {len(snap_paths)} segments in {time.time() - snapshot_start:.3f}s")

        concat_file = os.path.join(self.segment_dir, f"concat_{concat_id}.txt")
        with open(concat_file, "w", encoding="utf-8") as fh:
            for seg_name in concat_names:
                seg_path = os.path.join(self.segment_dir, seg_name).replace("\\", "/")
                fh.write(f"file '{seg_path}'\n")

        timestamp = time.strftime("%Y%m%d_%H%M%S")
        output_path = os.path.join(output_folder, f"Clip_{timestamp}.mp4")

        has_loopback = self.audio and self.audio.available
        has_mic = self.audio and self.audio.mic_available
        has_any_audio = has_loopback or has_mic

        loopback_wav = os.path.join(self.segment_dir, f"loopback_{concat_id}.wav") if has_loopback else None
        mic_wav = os.path.join(self.segment_dir, f"mic_{concat_id}.wav") if has_mic else None
        mixed_wav = os.path.join(self.segment_dir, f"mixed_{concat_id}.wav") if (has_loopback and has_mic) else None

        # Audio is reconstructed on the real timeline ending at save_time, for
        # the same real span the video settled on — no processing-delay trim is
        # needed (samples that arrived after save_time are excluded by their
        # timestamp, not by "last N bytes").
        audio_duration = total_duration
        t_end = save_time

        if has_loopback:
            a = self.audio
            with a._loopback_lock:
                lb_chunks, lb_bytes = len(a._loopback_chunks), a._loopback_bytes
            buf_span = lb_bytes / (a._rate * a._channels * a._sample_width)
            log(f"[{concat_id}] loopback: rate={a._rate} "
                f"stream_alive={a._stream_alive(a._loopback_stream)} "
                f"frames_received={a._loopback_frames} chunks={lb_chunks} "
                f"buf_bytes={lb_bytes} buf_span={buf_span:.3f}s "
                f"status_flags={a._loopback_status_flags}")
            ok = self.audio.save_wav(loopback_wav, t_end, audio_duration)
            log(f"[{concat_id}] save_wav(loopback) ok={ok} duration={audio_duration:.3f}s")
        if has_mic:
            a = self.audio
            with a._mic_lock:
                mic_chunks, mic_bytes = len(a._mic_chunks), a._mic_bytes
            buf_span = mic_bytes / (a._mic_rate * a._mic_channels * a._sample_width)
            log(f"[{concat_id}] mic: rate={a._mic_rate} "
                f"stream_alive={a._stream_alive(a._mic_stream)} "
                f"frames_received={a._mic_frames} chunks={mic_chunks} "
                f"buf_bytes={mic_bytes} buf_span={buf_span:.3f}s "
                f"status_flags={a._mic_status_flags}")
            ok = self.audio.save_mic_wav(mic_wav, t_end, audio_duration)
            log(f"[{concat_id}] save_mic_wav ok={ok} duration={audio_duration:.3f}s")

        log(f"[{concat_id}] audio_duration={audio_duration:.3f}s "
            f"has_loopback={has_loopback} has_mic={has_mic}")

        audio_wav = None
        if has_loopback and has_mic and os.path.exists(loopback_wav) and os.path.exists(mic_wav):
            audio_wav = mixed_wav
        elif has_loopback and loopback_wav and os.path.exists(loopback_wav):
            audio_wav = loopback_wav
        elif has_mic and mic_wav and os.path.exists(mic_wav):
            audio_wav = mic_wav

        if audio_wav and audio_wav != mixed_wav:
            mixed_wav = None

        if has_any_audio and audio_wav:
            video_only = os.path.join(self.segment_dir, f"video_{concat_id}.mp4")
            cmd_video = [
                FFMPEG, "-y",
                "-f", "concat", "-safe", "0",
                "-i", concat_file,
                "-ss", "0",
                "-t", str(total_duration),
                "-c", "copy",
                "-movflags", "+faststart",
                video_only,
            ]
        else:
            video_only = None
            cmd_video = [
                FFMPEG, "-y",
                "-f", "concat", "-safe", "0",
                "-i", concat_file,
                "-ss", "0",
                "-t", str(total_duration),
                "-c", "copy",
                "-movflags", "+faststart",
                output_path,
            ]

        def _log_result(label, result):
            err = result.stderr.decode("utf-8", errors="replace")[-2000:] if result.stderr else ""
            log(f"[{concat_id}] {label}: returncode={result.returncode}"
                + (f" stderr_tail={err!r}" if result.returncode != 0 else ""))

        def _run():
            run_start = time.time()
            success = False
            try:
                r = subprocess.run(
                    cmd_video, capture_output=True, timeout=30,
                    creationflags=0x08000000,
                )
                _log_result("video_concat", r)

                if mixed_wav and loopback_wav and mic_wav:
                    r = subprocess.run([
                        FFMPEG, "-y",
                        "-i", loopback_wav, "-i", mic_wav,
                        "-filter_complex",
                        "[0:a]aresample=48000,aformat=channel_layouts=stereo[a0];"
                        "[1:a]aresample=48000,aformat=channel_layouts=stereo[a1];"
                        "[a0][a1]amix=inputs=2:duration=longest:normalize=0",
                        "-ac", "2", "-ar", "48000",
                        mixed_wav,
                    ], capture_output=True, timeout=30,
                       creationflags=0x08000000)
                    _log_result("audio_mix", r)

                if video_only and audio_wav and os.path.exists(audio_wav) and os.path.exists(video_only):
                    r = subprocess.run([
                        FFMPEG, "-y",
                        "-i", video_only, "-i", audio_wav,
                        "-c:v", "copy",
                        "-c:a", "aac", "-b:a", "192k",
                        "-shortest",
                        "-movflags", "+faststart",
                        output_path,
                    ], capture_output=True, timeout=30,
                       creationflags=0x08000000)
                    _log_result("final_mux", r)

                if os.path.exists(output_path):
                    success = True
                    winsound.PlaySound(
                        "SystemExclamation",
                        winsound.SND_ALIAS | winsound.SND_ASYNC,
                    )
                    if on_success:
                        on_success()
            except Exception:
                log(f"[{concat_id}] EXCEPTION in _run:\n{traceback.format_exc()}")
            finally:
                log(f"[{concat_id}] done: output_exists={os.path.exists(output_path)} "
                    f"on_success_called={success} mux_time={time.time() - run_start:.3f}s "
                    f"total_save_replay_time={time.time() - save_time:.3f}s")
                for tmp in [concat_file, loopback_wav, mic_wav, mixed_wav, video_only] + snap_paths:
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
WS_EX_NOACTIVATE = 0x08000000


class NotificationBanner:
    def __init__(self, root, monitors, config):
        self.root = root
        self.monitors = monitors
        self.config = config
        self._win = None
        self._hide_id = None

    def show(self, text="Clip saved", duration_ms=3000):
        if self._win and self._win.winfo_exists():
            self._win.destroy()
        if self._hide_id:
            self.root.after_cancel(self._hide_id)

        mon_idx = min(self.config["monitor"], len(self.monitors) - 1)
        mon = self.monitors[mon_idx]

        self._win = tk.Toplevel(self.root)
        self._win.withdraw()
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

        self._win.update_idletasks()
        self._apply_window_styles()
        self._win.deiconify()
        self._win.after_idle(self._apply_window_styles)
        self._hide_id = self.root.after(duration_ms, self._hide)

    def _apply_window_styles(self):
        if not self._win or not self._win.winfo_exists():
            return
        hwnd = user32.GetParent(self._win.winfo_id())
        if not hwnd:
            hwnd = self._win.winfo_id()
        ex = user32.GetWindowLongPtrW(hwnd, GWL_EXSTYLE)
        user32.SetWindowLongPtrW(
            hwnd, GWL_EXSTYLE,
            ex | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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


# ─── Hotkey ──────────────────────────────────────────────────────────────────

HOTKEY_SAVE = 1


def parse_hotkey(hotkey_str):
    parts = [p.strip() for p in hotkey_str.split("+")]
    modifiers = MOD_NOREPEAT
    vk = 0
    for part in parts:
        if part in MODIFIER_MAP:
            modifiers |= MODIFIER_MAP[part]
        elif part in VK_MAP:
            vk = VK_MAP[part]
    if not vk or modifiers == MOD_NOREPEAT:
        return MOD_NOREPEAT | MOD_CONTROL | MOD_ALT, 0x52
    return modifiers, vk


class HotkeyManager:
    def __init__(self, root, on_save, modifiers=None, vk=None):
        self.root = root
        self.on_save = on_save
        self.modifiers = modifiers if modifiers is not None else (MOD_CONTROL | MOD_ALT | MOD_NOREPEAT)
        self.vk = vk if vk is not None else 0x52
        self._thread_id = None
        self._ready = threading.Event()
        self.registered = False
        self.last_error = None
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self):
        self._thread_id = ctypes.windll.kernel32.GetCurrentThreadId()
        self.registered = bool(user32.RegisterHotKey(None, HOTKEY_SAVE, self.modifiers, self.vk))
        if not self.registered:
            time.sleep(0.15)
            self.registered = bool(user32.RegisterHotKey(None, HOTKEY_SAVE, self.modifiers, self.vk))
        if not self.registered:
            self.last_error = ctypes.windll.kernel32.GetLastError()
        self._ready.set()
        msg = wt.MSG()
        while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
            if msg.message == WM_HOTKEY and msg.wParam == HOTKEY_SAVE:
                self.root.after(0, self.on_save)
        user32.UnregisterHotKey(None, HOTKEY_SAVE)

    def stop(self):
        self._ready.wait(timeout=2)
        if self._thread_id:
            ctypes.windll.user32.PostThreadMessageW(self._thread_id, 0x0012, 0, 0)
            self._thread.join(timeout=2)


# ─── Settings ────────────────────────────────────────────────────────────────

class SettingsWindow:
    def __init__(self, root, config, monitors, capture):
        self.root = root
        self.config = config
        self.monitors = monitors
        self.capture = capture
        self.win = None
        self.on_hotkey_change = None
        self.on_uninstall = None
        self._capturing_hotkey = False
        self._held_mods = set()

    def toggle(self):
        if self.win and self.win.winfo_exists():
            self.win.destroy()
            self.win = None
            return
        self._build()

    def _build(self):
        self.win = tk.Toplevel(self.root)
        self.win.title("Clip Recorder — Settings")
        self.win.geometry("420x510")
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
        tk.Label(row, text="Monitor:", bg=BG2, fg=FG, font=FONT,
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
        tk.Label(row, text="FPS:", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        fps_choices = FPS_OPTIONS if self.capture.has_ddagrab else [f for f in FPS_OPTIONS if f <= 60]
        current_fps = self.config.get("fps", 60)
        if current_fps not in fps_choices:
            current_fps = fps_choices[-1]
        self.fps_var = tk.StringVar(value=str(current_fps))
        om2 = tk.OptionMenu(row, self.fps_var, *[str(f) for f in fps_choices])
        om2.config(bg=BG3, fg=FG, activebackground=BG2, activeforeground=FG,
                   highlightthickness=0, font=FONT_S, relief="flat")
        om2["menu"].config(bg=BG3, fg=FG, activebackground=ACCENT, font=FONT_S)
        om2.pack(side="left")

        # Audio (WASAPI loopback)
        row = tk.Frame(cf, bg=BG2)
        row.pack(fill="x", padx=10, pady=(4, 2))
        tk.Label(row, text="System audio:", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        loopback_devices = AudioCapture.list_loopback_devices()
        current_loopback = self.capture.audio.device_name if self.capture.audio and self.capture.audio.available else ""
        loopback_choices = ["(Auto — system default)"] + loopback_devices
        self.loopback_var = tk.StringVar(value=current_loopback or loopback_choices[0])
        if loopback_devices:
            om_lb = tk.OptionMenu(row, self.loopback_var, *loopback_choices)
            om_lb.config(bg=BG3, fg=FG, activebackground=BG2, activeforeground=FG,
                         highlightthickness=0, font=FONT_S, relief="flat")
            om_lb["menu"].config(bg=BG3, fg=FG, activebackground=ACCENT, font=FONT_S)
            om_lb.pack(side="left", fill="x", expand=True)
        else:
            tk.Label(row, text="Not available", bg=BG2, fg=FG2, font=FONT_S,
                     anchor="w").pack(side="left", fill="x", expand=True)

        # Microphone
        row = tk.Frame(cf, bg=BG2)
        row.pack(fill="x", padx=10, pady=(2, 2))
        tk.Label(row, text="Mic:", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        mic_devices = AudioCapture.list_mic_devices()
        current_mic = self.capture.audio.mic_name if self.capture.audio and self.capture.audio.mic_available else ""
        mic_choices = ["(Auto — system default)"] + mic_devices
        self.mic_var = tk.StringVar(value=current_mic or mic_choices[0])
        if mic_devices:
            om_mic = tk.OptionMenu(row, self.mic_var, *mic_choices)
            om_mic.config(bg=BG3, fg=FG, activebackground=BG2, activeforeground=FG,
                          highlightthickness=0, font=FONT_S, relief="flat")
            om_mic["menu"].config(bg=BG3, fg=FG, activebackground=ACCENT, font=FONT_S)
            om_mic.pack(side="left", fill="x", expand=True)
        else:
            tk.Label(row, text="Not detected", bg=BG2, fg=FG2, font=FONT_S,
                     anchor="w").pack(side="left", fill="x", expand=True)

        # Encoder
        row = tk.Frame(cf, bg=BG2)
        row.pack(fill="x", padx=10, pady=2)
        tk.Label(row, text="Encoder:", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        enc = "NVENC (GPU)" if self.capture.has_nvenc else "x264 (CPU)"
        tk.Label(row, text=enc, bg=BG2, fg=ACCENT, font=FONT_B).pack(side="left")

        # Capture method
        row = tk.Frame(cf, bg=BG2)
        row.pack(fill="x", padx=10, pady=(2, 8))
        tk.Label(row, text="Capture:", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        cap_method = "DXGI (ddagrab)" if self.capture.has_ddagrab else "GDI (gdigrab)"
        tk.Label(row, text=cap_method, bg=BG2, fg=ACCENT, font=FONT_B).pack(side="left")

        # ── Replay ──
        self._section("Replay")
        rf = tk.Frame(self.win, bg=BG2, bd=1, relief="flat")
        rf.pack(fill="x", padx=15, pady=(0, 10))

        # Buffer
        row = tk.Frame(rf, bg=BG2)
        row.pack(fill="x", padx=10, pady=(8, 4))
        tk.Label(row, text="Duration (s):", bg=BG2, fg=FG, font=FONT,
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
        tk.Label(row, text="Folder:", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        self.folder_var = tk.StringVar(value=get_output_folder(self.config))
        tk.Entry(row, textvariable=self.folder_var, bg=BG3, fg=FG,
                 insertbackground=FG, font=FONT_S, relief="flat", bd=2,
                 ).pack(side="left", fill="x", expand=True, padx=(0, 5))
        tk.Button(row, text="...", command=self._browse_folder,
                  bg=BG3, fg=FG, relief="flat", font=FONT_S,
                  cursor="hand2", width=3).pack(side="left")

        # ── Hotkey ──
        self._section("Hotkey")
        kf = tk.Frame(self.win, bg=BG2, bd=1, relief="flat")
        kf.pack(fill="x", padx=15, pady=(0, 10))
        row = tk.Frame(kf, bg=BG2)
        row.pack(fill="x", padx=10, pady=6)
        tk.Label(row, text="Hotkey:", bg=BG2, fg=FG, font=FONT,
                 width=12, anchor="w").pack(side="left")
        self.hotkey_var = tk.StringVar(value=self.config.get("hotkey", "Ctrl+Alt+R"))
        self.hotkey_btn = tk.Button(
            row, textvariable=self.hotkey_var, command=self._start_hotkey_capture,
            bg=BG3, fg=ACCENT, font=FONT_B, relief="flat", padx=10, cursor="hand2",
        )
        self.hotkey_btn.pack(side="left")
        tk.Label(row, text="  Click to change", bg=BG2, fg=FG2,
                 font=FONT_S).pack(side="left")

        # ── Buttons ──
        bf = tk.Frame(self.win, bg=BG)
        bf.pack(fill="x", padx=15, pady=(8, 10))
        tk.Button(bf, text="Save", command=self._save,
                  bg=ACCENT, fg="#ffffff", font=FONT_B, relief="flat",
                  padx=15, cursor="hand2").pack(side="left")
        tk.Button(bf, text="Uninstall...", command=self._confirm_uninstall,
                  bg=BG3, fg=FG2, font=FONT, relief="flat",
                  padx=10, cursor="hand2").pack(side="right")

        self.win.lift()
        self.win.focus_force()

    def _section(self, text):
        tk.Label(self.win, text=text, bg=BG, fg=ACCENT, font=FONT_B,
                 anchor="w").pack(fill="x", padx=15, pady=(8, 2))

    def _browse_folder(self):
        folder = filedialog.askdirectory(
            title="Output folder",
            initialdir=self.folder_var.get(),
        )
        if folder:
            self.folder_var.set(folder)

    def _start_hotkey_capture(self):
        self._capturing_hotkey = True
        self._held_mods = set()
        self.hotkey_btn.config(bg=ACCENT, fg="#ffffff")
        self.hotkey_var.set("Press a key...")
        self.win.bind('<KeyPress>', self._on_hotkey_key)
        self.win.bind('<KeyRelease>', self._on_hotkey_release)
        self.win.focus_set()

    def _stop_hotkey_capture(self):
        self._capturing_hotkey = False
        self._held_mods = set()
        self.hotkey_btn.config(bg=BG3, fg=ACCENT)
        if self.win:
            self.win.unbind('<KeyPress>')
            self.win.unbind('<KeyRelease>')

    def _on_hotkey_key(self, event):
        if not self._capturing_hotkey:
            return 'break'
        if event.keysym == 'Escape':
            self.hotkey_var.set(self.config.get("hotkey", "Ctrl+Alt+R"))
            self._stop_hotkey_capture()
            return 'break'
        if event.keysym in ('Control_L', 'Control_R'):
            self._held_mods.add('Ctrl')
            return 'break'
        if event.keysym in ('Alt_L', 'Alt_R'):
            self._held_mods.add('Alt')
            return 'break'
        if event.keysym in ('Shift_L', 'Shift_R'):
            self._held_mods.add('Shift')
            return 'break'
        if event.keysym in ('Super_L', 'Super_R', 'Win_L', 'Win_R'):
            return 'break'
        key = KEYSYM_TO_KEY.get(event.keysym)
        if not key or not self._held_mods:
            return 'break'
        mods = [m for m in ("Ctrl", "Alt", "Shift") if m in self._held_mods]
        self.hotkey_var.set("+".join(mods + [key]))
        self._stop_hotkey_capture()
        return 'break'

    def _on_hotkey_release(self, event):
        if event.keysym in ('Control_L', 'Control_R'):
            self._held_mods.discard('Ctrl')
        elif event.keysym in ('Alt_L', 'Alt_R'):
            self._held_mods.discard('Alt')
        elif event.keysym in ('Shift_L', 'Shift_R'):
            self._held_mods.discard('Shift')
        return 'break'

    def _apply(self):
        mon_str = self.monitor_var.get()
        try:
            monitor = int(mon_str.split(":")[0]) - 1
        except Exception:
            monitor = 0

        loopback_sel = self.loopback_var.get()
        mic_sel = self.mic_var.get()
        loopback_name = "" if loopback_sel.startswith("(Auto") else loopback_sel
        mic_name = "" if mic_sel.startswith("(Auto") else mic_sel

        new_hotkey = self.hotkey_var.get()
        if new_hotkey == "Press a key...":
            new_hotkey = self.config.get("hotkey", "Ctrl+Alt+R")
            self.hotkey_var.set(new_hotkey)

        new = {
            "monitor": monitor,
            "fps": int(self.fps_var.get()),
            "buffer_seconds": int(self.buffer_var.get()),
            "output_folder": self.folder_var.get(),
            "loopback_device": loopback_name,
            "mic_device": mic_name,
            "hotkey": new_hotkey,
        }

        # Audio is rebuilt ONLY when a device changes. monitor/fps/buffer_seconds
        # affect only the video process — restarting audio for them needlessly
        # re-exposes the flaky WASAPI loopback (re)open that caused the reported
        # duration-change audio desync. So they take the video-only path.
        audio_changed = (
            new["loopback_device"] != self.config.get("loopback_device", "")
            or new["mic_device"] != self.config.get("mic_device", "")
        )
        video_changed = (
            new["monitor"] != self.config["monitor"]
            or new["fps"] != self.config.get("fps")
            or new["buffer_seconds"] != self.config.get("buffer_seconds")
        )
        hotkey_changed = new["hotkey"] != self.config.get("hotkey", "Ctrl+Alt+R")
        self.config.update(new)

        if audio_changed:
            self.capture.audio.stop()
            self.capture.audio = AudioCapture(
                max_seconds=max(BUFFER_OPTIONS),
                loopback_name=loopback_name,
                mic_name=mic_name,
            )
            self.capture.audio.start()

        if video_changed:
            self.capture.restart_video()

        if hotkey_changed and self.on_hotkey_change:
            self.on_hotkey_change()

    def _save(self):
        self._apply()
        save_config(self.config)

    def _close(self):
        if self._capturing_hotkey:
            self._stop_hotkey_capture()
        self.win.destroy()
        self.win = None

    def _confirm_uninstall(self):
        dlg = tk.Toplevel(self.win)
        dlg.title("Uninstall Clip Recorder")
        dlg.configure(bg=BG)
        dlg.resizable(False, False)
        dlg.attributes("-topmost", True)
        dlg.transient(self.win)
        dlg.grab_set()

        tk.Label(
            dlg, text="This will remove Clip Recorder from this computer.",
            bg=BG, fg=FG, font=FONT_B,
        ).pack(padx=20, pady=(20, 5), anchor="w")
        tk.Label(
            dlg, text="Your recorded clips are not touched, wherever they are saved.",
            bg=BG, fg=FG2, font=FONT_S,
        ).pack(padx=20, pady=(0, 15), anchor="w")

        bf = tk.Frame(dlg, bg=BG)
        bf.pack(padx=20, pady=(0, 20), fill="x")

        def cancel():
            dlg.destroy()

        def confirm():
            dlg.destroy()
            on_uninstall = self.on_uninstall
            self._close()
            if on_uninstall:
                on_uninstall()

        tk.Button(bf, text="Cancel", command=cancel,
                  bg=BG3, fg=FG, font=FONT, relief="flat",
                  padx=10, cursor="hand2").pack(side="left")
        tk.Button(bf, text="Uninstall", command=confirm,
                  bg="#cc3333", fg="#ffffff", font=FONT_B, relief="flat",
                  padx=15, cursor="hand2").pack(side="right")

        dlg.protocol("WM_DELETE_WINDOW", cancel)
        dlg.update_idletasks()
        w, h = dlg.winfo_reqwidth(), dlg.winfo_reqheight()
        x = self.win.winfo_x() + (self.win.winfo_width() - w) // 2
        y = self.win.winfo_y() + (self.win.winfo_height() - h) // 2
        dlg.geometry(f"+{x}+{y}")


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
                lambda item: f"Save clip ({self.config.get('hotkey', 'Ctrl+Alt+R')})",
                lambda: self.root.after(0, self.save_fn),
            ),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem(
                "Settings",
                lambda: self.root.after(0, self.settings.toggle),
            ),
            pystray.MenuItem(
                "Open folder",
                lambda: self.root.after(0, self._open_folder),
            ),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem(
                "Quit",
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
    # Single instance via Win32 mutex
    ctypes.windll.kernel32.CreateMutexW(None, True, "ClipRecorder_SingleInstance")
    if ctypes.windll.kernel32.GetLastError() == 183:  # ERROR_ALREADY_EXISTS
        sys.exit(0)

    # Clean orphan temp dirs from previous crashes
    try:
        tmp = tempfile.gettempdir()
        for d in os.listdir(tmp):
            if d.startswith("cliprec_"):
                shutil.rmtree(os.path.join(tmp, d), ignore_errors=True)
    except Exception:
        pass

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
            "ffmpeg.exe not found.\n\n"
            "Place ffmpeg.exe next to the application\n"
            "or install FFmpeg in PATH.",
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

    audio = AudioCapture(
        max_seconds=max(BUFFER_OPTIONS),
        loopback_name=config.get("loopback_device", ""),
        mic_name=config.get("mic_device", ""),
    )
    capture = FFmpegCapture(root, config, monitors, audio)
    banner = NotificationBanner(root, monitors, config)
    settings = SettingsWindow(root, config, monitors, capture)

    tray = None
    hotkeys = None

    def do_save():
        capture.save_replay(on_success=lambda: root.after(0, banner.show))

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

    def uninstall():
        nonlocal tray, hotkeys
        capture.cleanup()
        audio.cleanup()
        if hotkeys:
            hotkeys.stop()
        if tray:
            tray.stop()
        if getattr(sys, "frozen", False):
            exe_path = sys.executable
            desktop_lnk = os.path.join(os.path.expanduser("~"), "Desktop", "ClipRecorder.lnk")
            pid = os.getpid()

            def ps_quote(path):
                return path.replace("'", "''")

            ps_script = (
                f"Wait-Process -Id {pid} -Timeout 10 -ErrorAction SilentlyContinue;"
                "Start-Sleep -Milliseconds 500;"
                f"Remove-Item -LiteralPath '{ps_quote(exe_path)}' -Force -ErrorAction SilentlyContinue;"
                f"Remove-Item -LiteralPath '{ps_quote(CONFIG_FILE)}' -Force -ErrorAction SilentlyContinue;"
                f"Remove-Item -LiteralPath '{ps_quote(desktop_lnk)}' -Force -ErrorAction SilentlyContinue;"
            )
            subprocess.Popen(
                ["powershell", "-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", ps_script],
                creationflags=0x08000000,
            )
        root.destroy()
        sys.exit(0)

    hotkeys = HotkeyManager(root, do_save, *parse_hotkey(config.get("hotkey", "Ctrl+Alt+R")))

    def restart_hotkeys():
        nonlocal hotkeys
        if hotkeys:
            hotkeys.stop()
        requested = config.get("hotkey", "Ctrl+Alt+R")
        hotkeys = HotkeyManager(root, do_save, *parse_hotkey(requested))
        hotkeys._ready.wait(timeout=2)
        if not hotkeys.registered:
            err = hotkeys.last_error
            hotkeys.stop()
            import tkinter.messagebox
            if requested != "Ctrl+Alt+R":
                fallback = HotkeyManager(root, do_save, *parse_hotkey("Ctrl+Alt+R"))
                fallback._ready.wait(timeout=2)
                hotkeys = fallback
                if fallback.registered:
                    config["hotkey"] = "Ctrl+Alt+R"
                    save_config(config)
                    if settings.win:
                        settings.hotkey_var.set("Ctrl+Alt+R")
                    root.after(100, lambda: tkinter.messagebox.showwarning(
                        "ClipRecorder",
                        f"Couldn't register hotkey {requested} (Win32 error {err}).\n"
                        "It might already be in use by another application.\n"
                        "Hotkey reset to Ctrl+Alt+R.",
                    ))
                    return
            # requested was already Ctrl+Alt+R, or the fallback also failed —
            # leave config["hotkey"] untouched so a future attempt to change it
            # is correctly detected and retried, instead of being silently
            # swallowed forever.
            root.after(100, lambda: tkinter.messagebox.showwarning(
                "ClipRecorder",
                f"Couldn't register hotkey {requested} (Win32 error {err}).\n"
                "It might already be in use by another application.\n"
                "No hotkey is currently active — try a different combination in Settings.",
            ))

    settings.on_hotkey_change = restart_hotkeys
    settings.on_uninstall = uninstall
    tray = TrayIcon(root, config, capture, settings, shutdown, do_save)

    capture.start()
    root.mainloop()


if __name__ == "__main__":
    main()
