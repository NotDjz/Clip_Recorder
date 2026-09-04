"""
Clip Recorder — Replay screen recorder.

Continuous screen + audio capture via FFmpeg.
Ctrl+Alt+R saves the last X seconds as MP4.
"""

import atexit
import collections
import io
import json
import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import tkinter as tk
from tkinter import filedialog, ttk
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

def _find_ffmpeg():
    """Frozen: ffmpeg.exe is unpacked into BUNDLE_DIR. From source: this file
    lives in src/, while ffmpeg.exe sits at the repo root — so check the parent
    too before falling back to whatever is on PATH."""
    candidates = [os.path.join(BUNDLE_DIR, "ffmpeg.exe")]
    if not getattr(sys, "frozen", False):
        candidates.append(os.path.join(os.path.dirname(BUNDLE_DIR), "ffmpeg.exe"))
    for path in candidates:
        if os.path.exists(path):
            return path
    return "ffmpeg"


FFMPEG = _find_ffmpeg()

_log_lock = threading.Lock()

# The log ships to every user and is append-only, so cap it: past LOG_MAX_BYTES
# keep only the most recent half. Recent entries are what diagnosing a report
# needs, and this bounds the file instead of letting it grow forever.
LOG_MAX_BYTES = 2 * 1024 * 1024


def _trim_log_locked():
    """Caller must hold _log_lock."""
    try:
        if os.path.getsize(LOG_FILE) <= LOG_MAX_BYTES:
            return
        with open(LOG_FILE, "rb") as f:
            f.seek(-LOG_MAX_BYTES // 2, os.SEEK_END)
            f.readline()          # drop the partial line we landed in
            tail = f.read()
        with open(LOG_FILE, "wb") as f:
            f.write(b"[log truncated]\n" + tail)
    except Exception:
        pass


def log(msg):
    line = f"[{time.strftime('%Y-%m-%d %H:%M:%S')}] {msg}"
    try:
        with _log_lock:
            with open(LOG_FILE, "a", encoding="utf-8") as f:
                f.write(line + "\n")
                if f.tell() > LOG_MAX_BYTES:
                    trim = True
                else:
                    trim = False
            if trim:
                _trim_log_locked()
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

# Used by SettingsWindow and its uninstall dialog only — nothing else in the app
# reads these, so retuning them cannot disturb the tray icon or the banner (which
# carries its own colours). The greys are biased slightly green-cool rather than
# pure neutral: a pure grey reads as inherited, not chosen.
BG = "#191C1B"          # window ground
BG2 = "#232725"         # grouped surface
BG3 = "#2F3532"         # control surface / separators
FG = "#E6EAE7"          # text
FG2 = "#8B958F"         # labels, read-only facts
ACCENT = "#FF4A3D"      # the record dot and Save. Nothing else.
STRIP_TICK = "#49524D"  # buffer graduations; BG3 on BG2 is unreadable
STRIP_KEPT = "#2C2422"  # the slice the hotkey would write, tinted red
# Punched out by -transparentcolor to round the banner's corners, so it must
# never appear in the design itself.
BANNER_KEY = "#FF00FF"
FONT = ("Segoe UI", 10)
FONT_B = ("Segoe UI", 10, "bold")
FONT_S = ("Segoe UI", 9)
FONT_XS = ("Segoe UI", 8)
FONT_MONO = ("Consolas", 10)        # ships with Windows; used for the hotkey
FONT_STATUS = ("Segoe UI", 11, "bold")

# ─── Capture constants ──────────────────────────────────────────────────────

# The clip can only end at the last COMPLETE segment — the in-progress one is
# excluded because snapshotting a file mid-write yields a torn frame (confirmed:
# even remuxing it with -fflags +discardcorrupt recovers only ~0.01s and still
# decodes with errors). So whatever is still being written is lost, i.e. up to
# SEGMENT_DURATION of the moment right before the hotkey — exactly the part a
# replay recorder must not lose. Short segments keep that loss under a second
# (average ~0.5s) at the cost of more, smaller files and a 1s keyframe interval.
SEGMENT_DURATION = 1
# Each capture records its ffmpeg's PID beside its segments so the next launch
# can clean up after a crash — see _kill_orphan_ffmpeg().
FFMPEG_PID_FILE = "ffmpeg.pid"
# Temp-dir prefix for a capture's segments. The writer (mkdtemp) and the crash
# scanner in main() are the two halves of one contract, ~1000 lines apart:
# change it in only one and orphan cleanup silently stops working.
SEGMENT_DIR_PREFIX = "cliprec_"
# Budget for waiting on orphaned ffmpegs to exit, shared across the whole
# startup sweep. Per-directory it would be paid once per orphan, serialized
# before the tray icon appears — and a genuinely stuck one never self-clears,
# so it would be a permanent per-launch tax.
ORPHAN_KILL_WAIT_MS = 3000
# Never wait zero on an individual orphan, even once the shared budget is spent.
ORPHAN_KILL_FLOOR_MS = 250
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

            # Exact prefix FIRST, library resolver second. The resolver matches
            # with `in` (pyaudiowpatch 0.2.12.8) where this matches with
            # `startswith`: wider, but therefore less precise. With a default
            # output named "Headphones", `in` also matches a lower-indexed
            # "USB Headphones [Loopback]" and captures the wrong card silently —
            # the exact wrong-audio-source failure this file was bitten by once.
            # Each device is adopted last, after its rate and channel count have
            # been read: adopting first would leave the object describing
            # hardware it is not capturing if either lookup raised, and would
            # skip the resolver below because a device is already set.
            if not self._loopback_device:
                speakers = self._pa.get_device_info_by_index(
                    wasapi["defaultOutputDevice"])
                if speakers.get("isLoopbackDevice"):
                    self._rate = int(speakers["defaultSampleRate"])
                    self._channels = speakers["maxOutputChannels"]
                    self._loopback_device = speakers
                else:
                    for i in range(self._pa.get_device_count()):
                        dev = self._pa.get_device_info_by_index(i)
                        if (dev.get("name", "").startswith(speakers["name"])
                                and dev.get("isLoopbackDevice")):
                            self._rate = int(dev["defaultSampleRate"])
                            self._channels = dev["maxInputChannels"]
                            self._loopback_device = dev
                            break

            # The resolver catches what the prefix match misses (a loopback the
            # driver names differently from its output). Logged rather than
            # swallowed: a silent miss here downgrades the clip to mic-only with
            # nothing on screen or in the log to explain it.
            if not self._loopback_device:
                try:
                    dev = self._pa.get_default_wasapi_loopback()
                    self._rate = int(dev["defaultSampleRate"])
                    self._channels = dev["maxInputChannels"]
                    self._loopback_device = dev
                except Exception as e:      # LookupError / OSError / old lib
                    log(f"default loopback lookup failed: "
                        f"{type(e).__name__}: {e}")

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
        # Callback arrival times jitter by a few ms. Placing EVERY chunk at its
        # own rounded timestamp therefore punched a 1-5 ms hole (or overwrote
        # samples) at each chunk boundary — ~12 discontinuities/second, i.e.
        # audible crackle. So a run of chunks that arrived roughly on schedule is
        # kept strictly sample-contiguous, and we only jump to the real timestamp
        # when the divergence exceeds the tolerance — a genuine delivery gap
        # (WASAPI loopback going silent), which must stay silent. Comparing
        # against the ABSOLUTE real position each time means the contiguous run
        # can never drift further than the tolerance.
        tol = int(rate * 0.030)  # 30 ms
        wrote_any = False
        cursor = None            # where a continuing run would land next
        for t_arrival, data in chunks:
            n_frames = len(data) // frame_size
            if n_frames <= 0:
                continue
            real_pos = int(round((t_arrival - t_start) * rate)) - n_frames
            if cursor is None or abs(real_pos - cursor) > tol:
                place = real_pos       # first chunk, or a real gap → re-anchor
            else:
                place = cursor         # continuous run → no hole, no overwrite
            cursor = place + n_frames  # logical continuation, before clipping

            src_from, n, dst = 0, n_frames, place
            if dst < 0:                # partially (or fully) before the window
                src_from = -dst
                n -= src_from
                dst = 0
            if dst + n > total_frames:  # partially (or fully) after it
                n = total_frames - dst
            if n <= 0:
                continue
            out[dst * frame_size:(dst + n) * frame_size] = \
                data[src_from * frame_size:(src_from + n) * frame_size]
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


def _known_folder(folderid, fallback):
    """The HRESULT + CoTaskMemFree contract, written once."""
    try:
        path_ptr = ctypes.c_wchar_p()
        hr = ctypes.windll.shell32.SHGetKnownFolderPath(
            ctypes.byref(folderid), 0, None, ctypes.byref(path_ptr)
        )
        if hr == 0 and path_ptr.value:
            path = path_ptr.value
            ctypes.windll.ole32.CoTaskMemFree(path_ptr)
            return path
    except Exception:
        pass
    return fallback


def _get_videos_folder():
    return _known_folder(FOLDERID_VIDEOS,
                         os.path.join(os.path.expanduser("~"), "Videos"))


def get_output_folder(cfg):
    folder = cfg.get("output_folder") or ""
    if not folder:
        folder = os.path.join(_get_videos_folder(), "ClipRecorder")
    return folder


# {B97D20BB-F46A-4C97-BA10-5E3608430854} — the per-user Startup folder. Same
# reasoning as FOLDERID_VIDEOS above: the API follows a relocated folder, which
# a hand-built %APPDATA% path does not.
# {B4BFCC3A-DB2C-424C-B029-7FE99A87C641}
FOLDERID_DESKTOP = _GUID(
    0xB4BFCC3A, 0xDB2C, 0x424C,
    (ctypes.c_ubyte * 8)(0xB0, 0x29, 0x7F, 0xE9, 0x9A, 0x87, 0xC6, 0x41),
)


FOLDERID_STARTUP = _GUID(
    0xB97D20BB, 0xF46A, 0x4C97,
    (ctypes.c_ubyte * 8)(0xBA, 0x10, 0x5E, 0x36, 0x08, 0x43, 0x08, 0x54),
)


def _get_startup_folder():
    return _known_folder(FOLDERID_STARTUP, os.path.join(
        os.environ.get("APPDATA", ""), "Microsoft", "Windows",
        "Start Menu", "Programs", "Startup"))


def desktop_lnk_path():
    """Through the API, not os.path.expanduser: with OneDrive Known Folder
    Backup the desktop lives under ~/OneDrive/Desktop and the hand-built path
    does not exist, so the shortcut is silently never created or removed."""
    return os.path.join(
        _known_folder(FOLDERID_DESKTOP,
                      os.path.join(os.path.expanduser("~"), "Desktop")),
        "ClipRecorder.lnk")


def startup_lnk_path():
    return os.path.join(_get_startup_folder(), "ClipRecorder.lnk")


def startup_enabled():
    """The FILESYSTEM is the source of truth, deliberately — not a config key.

    Two sources would drift the moment someone deletes the shortcut by hand, and
    the app would keep insisting it starts with Windows when it does not."""
    return os.path.exists(startup_lnk_path())


def set_startup(enabled):
    """Create or remove the Startup shortcut. Only meaningful frozen: from source
    sys.executable is python.exe, and a shortcut to that launches nothing useful.

    setup.pyw carries its own copy. Importing the app from Setup is genuinely
    out: the module body pulls in pystray, PIL and pyaudiowpatch and binds
    CONFIG_FILE to Setup's own directory. A shared third module would work on
    the build side, but every test harness loads the app with
    spec_from_file_location, which puts nothing on sys.path — so each would
    break until it was taught otherwise. Keeping ~30 lines of ctypes constants
    twice is the chosen trade, not an unavoidable one."""
    lnk = startup_lnk_path()
    try:
        if not enabled:
            if os.path.exists(lnk):
                os.remove(lnk)
            return True
        if not getattr(sys, "frozen", False):
            return False
        target = sys.executable
        # Paths travel in the ENVIRONMENT, never interpolated into the script.
        # Quoting them is not fixable by escaping: PowerShell accepts U+2018,
        # U+2019, U+201A and U+201B as string delimiters, AND the console
        # codepage rewrites those to a plain apostrophe in transit — so a quote
        # can appear *after* the escape ran and close the literal. Measured.
        ps = (
            "$s = New-Object -ComObject WScript.Shell;"
            "$sc = $s.CreateShortcut($env:CR_LNK);"
            "$sc.TargetPath = $env:CR_TARGET;"
            "$sc.WorkingDirectory = Split-Path -Parent $env:CR_TARGET;"
            "$sc.IconLocation = $env:CR_TARGET;"
            "$sc.Save()"
        )
        r = subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", ps],
            capture_output=True, timeout=10, creationflags=0x08000000,
            env={**os.environ, "CR_LNK": lnk, "CR_TARGET": target},
        )
        # PowerShell can refuse New-Object -ComObject without raising here
        # (Constrained Language Mode, AppLocker): a nonzero exit is the only
        # signal, and without logging it the checkbox just reverts in silence.
        if r.returncode != 0:
            log(f"startup shortcut refused (rc={r.returncode}): "
                f"{r.stderr[-300:].decode('utf-8', 'replace').strip()}")
        return os.path.exists(lnk)
    except Exception as e:
        log(f"startup shortcut ({'on' if enabled else 'off'}) failed: "
            f"{type(e).__name__}: {e}")
        return False


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


# ─── Kill-on-close job object ────────────────────────────────────────────────

# Prevention for the orphan problem _kill_orphan_ffmpeg() cleans up after: a
# force-killed or crashed parent does not take its children with it. A job with
# KILL_ON_JOB_CLOSE makes the OS do it — when this process dies by ANY means
# (taskkill /F, Task Manager, an access violation in a C extension), our handle
# to the job closes and every process in it goes too. Children inherit job
# membership, so this covers the capture ffmpeg AND save_replay()'s three, which
# are recorded in no PID file and were previously unrecoverable.
#
# BREAKAWAY_OK exists solely so uninstall()'s PowerShell helper can escape with
# CREATE_BREAKAWAY_FROM_JOB: it waits for THIS process to exit before deleting
# the exe, so being killed with us would silently break uninstall entirely. It
# must be an explicit opt-in per child — SILENT_BREAKAWAY_OK would let the
# ffmpegs escape too and defeat the whole mechanism.
JOB_OBJECT_LIMIT_BREAKAWAY_OK = 0x00000800
JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
JOBOBJECT_EXTENDED_LIMIT_INFORMATION_CLASS = 9
CREATE_BREAKAWAY_FROM_JOB = 0x01000000

_job_handle = None      # never closed on purpose: closing it kills the job


class _JOBOBJECT_BASIC_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [("PerProcessUserTimeLimit", wt.LARGE_INTEGER),
                ("PerJobUserTimeLimit", wt.LARGE_INTEGER),
                ("LimitFlags", wt.DWORD),
                ("MinimumWorkingSetSize", ctypes.c_size_t),
                ("MaximumWorkingSetSize", ctypes.c_size_t),
                ("ActiveProcessLimit", wt.DWORD),
                ("Affinity", ctypes.c_size_t),
                ("PriorityClass", wt.DWORD),
                ("SchedulingClass", wt.DWORD)]


class _IO_COUNTERS(ctypes.Structure):
    _fields_ = [("ReadOperationCount", ctypes.c_ulonglong),
                ("WriteOperationCount", ctypes.c_ulonglong),
                ("OtherOperationCount", ctypes.c_ulonglong),
                ("ReadTransferCount", ctypes.c_ulonglong),
                ("WriteTransferCount", ctypes.c_ulonglong),
                ("OtherTransferCount", ctypes.c_ulonglong)]


class _JOBOBJECT_EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [("BasicLimitInformation", _JOBOBJECT_BASIC_LIMIT_INFORMATION),
                ("IoInfo", _IO_COUNTERS),
                ("ProcessMemoryLimit", ctypes.c_size_t),
                ("JobMemoryLimit", ctypes.c_size_t),
                ("PeakProcessMemoryUsed", ctypes.c_size_t),
                ("PeakJobMemoryUsed", ctypes.c_size_t)]


def _ensure_kill_on_close_job():
    """Put this process in a job whose children die with it. Idempotent.

    Called from FFmpegCapture.__init__ rather than main() so the test harnesses
    — which construct FFmpegCapture directly and never reach main() — get the
    same protection; a killed harness used to leak an ffmpeg that only a later
    launch of the real app would reap.

    Best-effort by design: assignment fails if we are already inside a
    non-nestable job, so _kill_orphan_ffmpeg() stays as the backstop."""
    global _job_handle
    if _job_handle is not None:
        return
    try:
        k32 = ctypes.windll.kernel32
        # These signatures are load-bearing, not decoration. HANDLE is
        # pointer-sized, and GetCurrentProcess returns the pseudo-handle
        # (HANDLE)-1 == 0xFFFFFFFFFFFFFFFF: with ctypes' default int marshalling
        # that argument raises OverflowError, which the except below would
        # swallow — leaving a job that exists but contains nothing. Declared
        # here rather than globally because no other call site uses them.
        k32.CreateJobObjectW.argtypes = [ctypes.c_void_p, wt.LPCWSTR]
        k32.CreateJobObjectW.restype = wt.HANDLE
        k32.GetCurrentProcess.restype = wt.HANDLE
        k32.SetInformationJobObject.argtypes = [wt.HANDLE, ctypes.c_int,
                                                ctypes.c_void_p, wt.DWORD]
        k32.SetInformationJobObject.restype = wt.BOOL
        k32.AssignProcessToJobObject.argtypes = [wt.HANDLE, wt.HANDLE]
        k32.AssignProcessToJobObject.restype = wt.BOOL
        job = k32.CreateJobObjectW(None, None)
        if not job:
            return
        info = _JOBOBJECT_EXTENDED_LIMIT_INFORMATION()
        info.BasicLimitInformation.LimitFlags = (
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK)
        if not k32.SetInformationJobObject(
                job, JOBOBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                ctypes.byref(info), ctypes.sizeof(info)):
            k32.CloseHandle(job)
            return
        if not k32.AssignProcessToJobObject(job, k32.GetCurrentProcess()):
            log(f"job object not assigned (err={k32.GetLastError()}) — "
                "falling back to PID-file orphan cleanup")
            k32.CloseHandle(job)
            return
        _job_handle = job
        log("job object active: ffmpeg children will die with this process")
    except Exception as e:
        # Never block capture over this — but never fail silently either: a
        # swallowed OverflowError here is exactly how this shipped inert the
        # first time, with the job created and the process never in it.
        log(f"job object setup failed: {type(e).__name__}: {e}")


# ─── FFmpeg Capture ──────────────────────────────────────────────────────────

class FFmpegCapture:
    def __init__(self, root, config, monitors, audio_capture):
        self.root = root
        self.config = config
        self.monitors = monitors
        self.audio = audio_capture
        self.proc = None
        # Before any ffmpeg is spawned — including the probes just below.
        _ensure_kill_on_close_job()
        self.segment_dir = tempfile.mkdtemp(prefix=SEGMENT_DIR_PREFIX)
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
        # Record the PID so a later launch can clean this up if we are killed
        # before stop() runs (see _kill_orphan_ffmpeg).
        try:
            with open(os.path.join(self.segment_dir, FFMPEG_PID_FILE), "w") as f:
                f.write(str(self.proc.pid))
        except Exception:
            pass

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

    def is_running(self):
        """A method, not a property: poll() reaps the child, so it is not free
        of side effects. Note this is NOT the negation of _check_health()'s
        test, which asks the different question 'did it exist and die?'."""
        return bool(self.proc and self.proc.poll() is None)

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
        # The window must start where the OLDEST SELECTED segment *opens*, since
        # that segment's content is included. A segment opens when the previous
        # one closes, so in the normal path the anchor is the mtime of the
        # segment just before the selection (`complete[n - count - 1]`).
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
            # Not enough history yet (just after a start/restart): everything is
            # selected, so there is no earlier segment to anchor on. Estimate the
            # oldest segment's OPEN time as its mtime minus one segment. Using its
            # mtime directly — as this did — anchors the audio one whole segment
            # late, i.e. exactly SEGMENT_DURATION of A/V offset on any save made
            # before the buffer has refilled (1s now; it was 5s with 5s segments).
            count = n
            total_duration = save_time - (complete[0][1] - SEGMENT_DURATION)

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
                    play_save_sound()
                    if on_success:
                        on_success(round(total_duration))
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


# ─── Save sound ──────────────────────────────────────────────────────────────

# Low and short on purpose: this fires while you are still playing, so it has to
# register without cutting through the game the way SystemExclamation did — that
# alias is also what Windows uses for error dialogs, which is not what a saved
# clip is. Synthesised rather than shipped: no asset in the .spec, no licence.
# (frequency, amplitude, second-harmonic ratio) per voice
SAVE_TONE_VOICES = ((220.0, 0.34, 0.5), (330.0, 0.18, 0.0))
SAVE_TONE_MS = 200
_save_sound_wav = None


def _save_sound():
    """WAV bytes for the save chime, rendered once and kept.

    The 4 ms attack matters: starting a sine at full amplitude puts a step in the
    waveform, which is audible as a click before the note."""
    global _save_sound_wav
    if _save_sound_wav is not None:
        return _save_sound_wav
    rate = 48000
    n = int(rate * SAVE_TONE_MS / 1000)
    attack = max(1, int(0.004 * rate))
    buf = [0.0] * n
    for freq, amp, harm in SAVE_TONE_VOICES:
        for i in range(n):
            e = min(1.0, i / attack) * math.exp(-5.0 * i / n)
            buf[i] += amp * e * (math.sin(2 * math.pi * freq * i / rate)
                                 + harm * math.sin(4 * math.pi * freq * i / rate))
    bio = io.BytesIO()
    with wave.open(bio, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(b"".join(
            struct.pack("<h", int(max(-1.0, min(1.0, v)) * 32767)) for v in buf))
    _save_sound_wav = bio.getvalue()
    return _save_sound_wav


def play_save_sound():
    """winsound REFUSES SND_ASYNC with SND_MEMORY (RuntimeError: cannot play
    asynchronously from memory), so the play is synchronous and gets its own
    thread — otherwise it would sit in front of the notification banner for the
    length of the sound."""
    def _play():
        try:
            winsound.PlaySound(_save_sound(), winsound.SND_MEMORY)
        except Exception as e:
            log(f"save sound failed: {type(e).__name__}: {e}")
    threading.Thread(target=_play, daemon=True).start()


# ─── Notification Banner ────────────────────────────────────────────────────

DWMWA_USE_IMMERSIVE_DARK_MODE = 20      # 19 on Win10 builds before 20H1
DWMWA_CAPTION_COLOR = 35                # Win11 22000+; overrides the accent
DWMWA_TEXT_COLOR = 36
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

    def show(self, seconds, text="Clip saved", duration_ms=3000):
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
        ground = BANNER_KEY
        try:
            self._win.attributes("-transparentcolor", BANNER_KEY)
        except tk.TclError:
            # Without the key colour the pill cannot be cut out, so fall back to
            # a plain rectangle — leaving it magenta would put a bright block
            # over the game instead of a banner.
            ground = BG
        self._win.configure(bg=ground)

        w, h = 250, 40
        x = mon["x"] + mon["w"] - w - 24
        y = mon["y"] + 24
        self._win.geometry(f"{w}x{h}+{x}+{y}")

        c = tk.Canvas(self._win, width=w, height=h, bg=ground,
                      highlightthickness=0)
        c.pack()
        r = h // 2                      # a pill: two circles and a rectangle
        c.create_oval(0, 0, h, h, fill=BG, outline="")
        c.create_oval(w - h, 0, w, h, fill=BG, outline="")
        c.create_rectangle(r, 0, w - r, h, fill=BG, outline="")
        # The record dot, resolved. No tick: it repeats "saved" in a second
        # modality, and the accent is worth more spent once.
        PAD, DOT = 20, 4
        c.create_oval(PAD - 1, h // 2 - DOT, PAD + 7, h // 2 + DOT,
                      fill=ACCENT, outline="")
        c.create_text(38, h // 2, text=text, anchor="w", fill=FG,
                      font=FONT_STATUS)
        # The clip's real length, which runs buffer_seconds to +SEGMENT_DURATION
        # because whole segments are selected — so 60s often reads 61s. Showing
        # the true figure beats rounding it to the setting.
        c.create_text(w - PAD, h // 2, text=f"{seconds}s", anchor="e",
                      fill=FG2, font=FONT_MONO)

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

def _colorref(hex_rgb):
    """#RRGGBB -> Win32 COLORREF, which is 0x00BBGGRR: the bytes run backwards."""
    r, g, b = (int(hex_rgb[i:i + 2], 16) for i in (1, 3, 5))
    return (b << 16) | (g << 8) | r


def _use_dark_titlebar(win):
    """Paint the title bar to match the window instead of the desktop accent.

    Dark mode alone is NOT enough: with "show accent colour on title bars"
    switched on, Windows paints the ACTIVE window's bar in the accent regardless
    — which is why an early attempt here looked fixed. It had only been captured
    while the window was inactive, and inactive bars never take the accent.
    DWMWA_CAPTION_COLOR is what actually overrides it (Win11 22000+; older
    builds return an error we drop, and keep the dark-mode bar)."""
    try:
        win.update_idletasks()
        hwnd = user32.GetParent(win.winfo_id()) or win.winfo_id()
        for attr, value in ((DWMWA_USE_IMMERSIVE_DARK_MODE, 1),
                            (DWMWA_CAPTION_COLOR, _colorref(BG)),
                            (DWMWA_TEXT_COLOR, _colorref(FG))):
            v = ctypes.c_int(value)
            ctypes.windll.dwmapi.DwmSetWindowAttribute(
                hwnd, attr, ctypes.byref(v), ctypes.sizeof(v))
    except Exception as e:
        # Cosmetic only, so never fatal — but logged, not swallowed: this file
        # already shipped one silently inert ctypes block (see the job object).
        log(f"dark title bar failed: {type(e).__name__}: {e}")


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
        self._status_gen = 0

    def toggle(self):
        if self.win and self.win.winfo_exists():
            self.win.destroy()
            self.win = None
            return
        self._build()

    def _style_comboboxes(self):
        """ttk.Combobox is the only dropdown tkinter can render dark, but it only
        takes colour on the 'clam' theme, and its popup is a classic Listbox that
        ttk.Style cannot reach — that one goes through the option database."""
        style = ttk.Style(self.win)
        try:
            style.theme_use("clam")
        except tk.TclError:
            pass                                # fall back to whatever is there
        style.configure("CR.TCombobox", fieldbackground=BG3, background=BG3,
                        foreground=FG, arrowcolor=FG2, bordercolor=BG3,
                        lightcolor=BG3, darkcolor=BG3, relief="flat", padding=3)
        style.map("CR.TCombobox",
                  fieldbackground=[("readonly", BG3)],
                  foreground=[("readonly", FG)],
                  selectbackground=[("readonly", BG3)],
                  selectforeground=[("readonly", FG)])
        for opt, val in (("background", BG3), ("foreground", FG),
                         ("selectBackground", ACCENT),
                         ("selectForeground", "#ffffff")):
            self.win.option_add(f"*TCombobox*Listbox.{opt}", val)

    def _group(self, title):
        """A titled block. The title is dim and sentence case: the accent is spent
        on the record dot and Save, not on decorating three headings."""
        tk.Label(self.win, text=title, bg=BG, fg=FG2, font=FONT_S,
                 anchor="w").pack(fill="x", padx=16, pady=(12, 4))
        frame = tk.Frame(self.win, bg=BG2)
        frame.pack(fill="x", padx=16)
        return frame

    def _row(self, parent, label):
        row = tk.Frame(parent, bg=BG2)
        row.pack(fill="x", padx=12, pady=4)
        tk.Label(row, text=label, bg=BG2, fg=FG2, font=FONT,
                 width=14, anchor="w").pack(side="left")
        return row

    def _combo(self, row, var, values, width=0):
        """width=0 takes the rest of the row; a fixed width is for the short
        numeric pickers, which look absurd stretched across it."""
        cb = ttk.Combobox(row, textvariable=var, values=values,
                          state="readonly", style="CR.TCombobox", font=FONT_S)
        if width:
            cb.config(width=width)
        cb.pack(side="left", fill="x", expand=not width)

    def _buffer_secs(self):
        """The duration both halves of the status block have to agree on. The
        combobox is readonly, so only a hand-edited config.json can make this
        unparseable — fall back to the same default the rest of the app uses."""
        for read in (self.buffer_var.get,
                     lambda: self.config.get("buffer_seconds")):
            try:
                return int(read())
            except (ValueError, TypeError, tk.TclError):
                pass            # a hand-edited null reaches here, not just ""
        return DEFAULTS["buffer_seconds"]

    def _draw_strip(self, *_):
        """The rolling buffer: the whole 120s memory, with the window the hotkey
        would write marked in red. Redrawn on a duration change, never animated —
        an animation loop would burn CPU for as long as the window is open."""
        c = self.strip
        if not c.winfo_exists():            # destroyed by a previous toggle()
            return
        c.delete("all")
        w, h = c.winfo_width(), c.winfo_height()
        if w <= 1:
            return                              # not laid out yet
        span = max(BUFFER_OPTIONS)
        kept = min(self._buffer_secs(), span)
        px = w / span
        x0 = w - kept * px
        c.create_rectangle(x0, 0, w, h, fill=STRIP_KEPT, outline="")
        for s in range(0, span + 1, 2):
            x = w - s * px
            c.create_line(x, h - 8, x, h, fill=STRIP_TICK)
        for s in range(0, span + 1, 30):
            x = w - s * px
            c.create_line(x, h - 12, x, h, fill=FG2)
        c.create_line(x0, 0, x0, h, fill=ACCENT)
        c.create_line(w - 1, 0, w - 1, h, fill=ACCENT, width=2)

    def _refresh_status(self, *_):
        """Drives the WHOLE status block, not just the sentence.

        Read once at build time, the dot would still say "Stopped" after
        _check_health() restarted ffmpeg a second later, and the facts line would
        show the saved fps while the combobox above it showed the pending one —
        two numbers for the same thing, side by side."""
        if not self.win or not self.win.winfo_exists():
            return
        key = self.hotkey_var.get()
        if key == "Press a key...":             # mid-capture; show the real one
            key = self.config.get("hotkey", "Ctrl+Alt+R")
        self.sentence_var.set(
            f"{key} saves the last {self._buffer_secs()} seconds.")

        self._refresh_liveness()
        self.facts_label.config(text="     ".join([
            "NVENC" if self.capture.has_nvenc else "x264",
            "DXGI" if self.capture.has_ddagrab else "GDI",
            f"{self.fps_var.get()} fps",
        ]))
        self._draw_strip()

    def _refresh_liveness(self):
        """The dot and the state word: the only two things here that change
        without the user touching a control."""
        running = self.capture.is_running()
        self.dot_label.config(fg=ACCENT if running else FG2)
        self.state_label.config(text="  Recording" if running else "  Stopped")

    def _poll_status(self, gen):
        """Liveness is not a build-time fact: _check_health() restarts a dead
        ffmpeg about a second later.

        Reschedules BEFORE doing the work, so one exception cannot silently
        freeze the block for good in a windowed .pyw with no console. Carries a
        generation token because reopening Settings inside the tick leaves
        self.win pointing at a live window again — the old chain would survive
        that check and poll forever alongside the new one. Same guard as the
        audio _heal_gen.

        It deliberately does NOT redraw the strip: that depends only on
        buffer_var, which no timer can change."""
        if gen != self._status_gen or not self.win or not self.win.winfo_exists():
            return
        self.root.after(1000, self._poll_status, gen)
        self._refresh_liveness()

    def _build(self):
        self.win = tk.Toplevel(self.root)
        self.win.title("Clip Recorder — Settings")
        self.win.geometry("460x615")
        self.win.resizable(False, False)
        self.win.configure(bg=BG)
        self.win.attributes("-topmost", True)
        self._icon_photo = ImageTk.PhotoImage(create_tray_icon_image(32))
        self.win.iconphoto(False, self._icon_photo)
        self.win.protocol("WM_DELETE_WINDOW", self._close)
        self._style_comboboxes()
        _use_dark_titlebar(self.win)

        # The status block below reads fps_var and traces buffer_var and
        # hotkey_var, so these four have to exist before it is built. The other
        # three vars are created next to the widget that owns them.
        mon_labels = []
        for i, m in enumerate(self.monitors):
            tag = " ★" if m["primary"] else ""
            # "N: name (WxH)" is a PARSED format — _apply() does
            # int(value.split(":")[0]) - 1. Changing the separator silently
            # selects the wrong monitor.
            mon_labels.append(f"{i+1}: {m['name']} ({m['w']}×{m['h']}){tag}")
        self.monitor_var = tk.StringVar(
            value=mon_labels[min(self.config["monitor"], len(mon_labels) - 1)]
        )
        fps_choices = FPS_OPTIONS if self.capture.has_ddagrab else [f for f in FPS_OPTIONS if f <= 60]
        current_fps = self.config.get("fps", 60)
        if current_fps not in fps_choices:
            current_fps = fps_choices[-1]
        self.fps_var = tk.StringVar(value=str(current_fps))
        self.buffer_var = tk.StringVar(
            value=str(self.config.get("buffer_seconds", 30)))
        self.hotkey_var = tk.StringVar(
            value=self.config.get("hotkey", "Ctrl+Alt+R"))
        self.sentence_var = tk.StringVar()

        # ── Status: is it running, and what does the key do? ──
        st = tk.Frame(self.win, bg=BG)
        st.pack(fill="x", padx=16, pady=(14, 0))
        top = tk.Frame(st, bg=BG)
        top.pack(fill="x")
        # Encoder and capture method are hardware FACTS, not settings — they read
        # out here instead of sitting in rows identical to the ones you can change.
        # All three are filled by _refresh_status() so they stay current.
        self.dot_label = tk.Label(top, text="●", bg=BG, font=FONT_S)
        self.dot_label.pack(side="left")
        self.state_label = tk.Label(top, bg=BG, fg=FG, font=FONT_STATUS)
        self.state_label.pack(side="left")
        self.facts_label = tk.Label(top, bg=BG, fg=FG2, font=FONT_XS)
        self.facts_label.pack(side="right")

        self.strip = tk.Canvas(st, height=26, bg=BG2, highlightthickness=1,
                               highlightbackground=BG3)
        self.strip.pack(fill="x", pady=(11, 9))
        self.strip.bind("<Configure>", self._draw_strip)

        tk.Label(st, textvariable=self.sentence_var, bg=BG, fg=FG,
                 font=FONT, anchor="w").pack(fill="x")

        # ── Capture ──
        cf = self._group("Capture")
        self._combo(self._row(cf, "Monitor"), self.monitor_var, mon_labels)
        row = self._row(cf, "Frame rate")
        self._combo(row, self.fps_var, [str(f) for f in fps_choices], width=6)
        tk.Label(row, text="  fps", bg=BG2, fg=FG2, font=FONT).pack(side="left")

        # ── Audio ──
        # "(Auto" is a PARSED prefix — _apply() does value.startswith("(Auto") to
        # mean "empty string in config", i.e. auto-detect. Renaming it silently
        # pins the config to a device literally named "(Auto — system default)".
        af = self._group("Audio")
        loopback_devices = AudioCapture.list_loopback_devices()
        current_loopback = self.capture.audio.device_name if self.capture.audio and self.capture.audio.available else ""
        loopback_choices = ["(Auto — system default)"] + loopback_devices
        self.loopback_var = tk.StringVar(value=current_loopback or loopback_choices[0])
        row = self._row(af, "System sound")
        if loopback_devices:
            self._combo(row, self.loopback_var, loopback_choices)
        else:
            tk.Label(row, text="Not available", bg=BG2, fg=FG2, font=FONT_S,
                     anchor="w").pack(side="left", fill="x", expand=True)

        mic_devices = AudioCapture.list_mic_devices()
        current_mic = self.capture.audio.mic_name if self.capture.audio and self.capture.audio.mic_available else ""
        mic_choices = ["(Auto — system default)"] + mic_devices
        self.mic_var = tk.StringVar(value=current_mic or mic_choices[0])
        row = self._row(af, "Microphone")
        if mic_devices:
            self._combo(row, self.mic_var, mic_choices)
        else:
            tk.Label(row, text="Not detected", bg=BG2, fg=FG2, font=FONT_S,
                     anchor="w").pack(side="left", fill="x", expand=True)

        # ── Replay ──
        rf = self._group("Replay")
        row = self._row(rf, "Keep the last")
        self._combo(row, self.buffer_var,
                    [str(b) for b in BUFFER_OPTIONS], width=6)
        tk.Label(row, text="  seconds", bg=BG2, fg=FG2,
                 font=FONT).pack(side="left")

        row = self._row(rf, "Save clips to")
        self.folder_var = tk.StringVar(value=get_output_folder(self.config))
        tk.Entry(row, textvariable=self.folder_var, bg=BG3, fg=FG,
                 insertbackground=FG, font=FONT_S, relief="flat", bd=4,
                 ).pack(side="left", fill="x", expand=True, padx=(0, 6))
        tk.Button(row, text="Browse", command=self._browse_folder,
                  bg=BG3, fg=FG2, relief="flat", font=FONT_S,
                  cursor="hand2", padx=8).pack(side="left")

        # Stays a classic tk.Button: _start_hotkey_capture() recolours it with
        # .config(bg=, fg=), which a ttk widget refuses outright.
        row = self._row(rf, "Hotkey")
        self.hotkey_btn = tk.Button(
            row, textvariable=self.hotkey_var, command=self._start_hotkey_capture,
            bg=BG3, fg=ACCENT, font=FONT_MONO, relief="flat", padx=10,
            cursor="hand2",
        )
        self.hotkey_btn.pack(side="left")
        tk.Label(row, text="  click to change — combination",
                 bg=BG2, fg=FG2,
                 font=FONT_S).pack(side="left")

        # ── Startup ──
        # No config key: the shortcut's existence IS the state. Storing it twice
        # would drift the moment someone deletes it by hand, and the app would go
        # on claiming it starts with Windows when it does not.
        sf = self._group("Startup")
        row = tk.Frame(sf, bg=BG2)
        row.pack(fill="x", padx=12, pady=6)
        self.startup_var = tk.BooleanVar(value=startup_enabled())
        frozen = getattr(sys, "frozen", False)
        tk.Checkbutton(
            row, text="Start with Windows", variable=self.startup_var,
            bg=BG2, fg=FG if frozen else FG2, selectcolor=BG3,
            activebackground=BG2, activeforeground=FG, font=FONT,
            highlightthickness=0, bd=0, cursor="hand2" if frozen else "arrow",
            state="normal" if frozen else "disabled",
        ).pack(side="left")
        if not frozen:
            # From source sys.executable is python.exe; a shortcut to that starts
            # nothing useful, so the control would be a lie rather than a setting.
            tk.Label(row, text="  built app only", bg=BG2, fg=FG2,
                     font=FONT_S).pack(side="left")

        # ── Footer: Save is primary, Uninstall keeps its distance ──
        tk.Frame(self.win, bg=BG3, height=1).pack(fill="x", padx=16, pady=(18, 0))
        bf = tk.Frame(self.win, bg=BG)
        bf.pack(fill="x", padx=16, pady=(14, 0))
        tk.Button(bf, text="Save changes", command=self._save,
                  bg=ACCENT, fg="#ffffff", font=FONT_B, relief="flat",
                  padx=18, pady=4, cursor="hand2").pack(side="left")
        tk.Button(bf, text="Uninstall…", command=self._confirm_uninstall,
                  bg=BG, fg=FG2, font=FONT_S, relief="flat",
                  padx=6, cursor="hand2").pack(side="right")

        for var in (self.buffer_var, self.hotkey_var, self.fps_var):
            var.trace_add("write", self._refresh_status)
        # Fill the block once here: the traces only fire on a CHANGE, and
        # _poll_status refreshes liveness alone — without this the sentence and
        # the facts line open blank.
        self._refresh_status()
        self._status_gen += 1           # retires any poller from a previous open
        self._poll_status(self._status_gen)

        self.win.lift()
        self.win.focus_force()

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

        # Reconcile the Startup shortcut with the checkbox. Goes through Save
        # like every other control: the window has ONE button by design, and
        # CLAUDE.md forbids re-adding a second one.
        # No `!= startup_enabled()` guard: existence alone cannot tell a shortcut
        # pointing at THIS exe from one left by an older install, and the guard
        # made the stale case unfixable from here. Creating is idempotent and
        # re-points the target, so just do it whenever the box is ticked.
        wanted = self.startup_var.get()
        if wanted or startup_enabled():
            # Off the Tk thread: set_startup spawns powershell with timeout=10,
            # and _apply() runs on the UI thread — done inline this freezes the
            # window, the tray and every pending after() callback for seconds,
            # on every Save, even one that only changed the frame rate.
            threading.Thread(target=set_startup, args=(wanted,),
                             daemon=True).start()

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
        _use_dark_titlebar(dlg)     # else it opens a light bar on top of a dark one
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

def _kill_orphan_ffmpeg(seg_dir, deadline=None):
    """Terminate the ffmpeg left behind by a force-killed / crashed instance.

    A capture child does not die with its parent: it keeps writing to temp AND
    keeps an NVENC session open. Consumer GPUs allow only a handful of those, so
    a few orphans make every later launch fail to encode — no segments, no clips,
    and nothing on screen to explain it (observed: four orphans, capture silently
    producing nothing).

    Only PIDs this app recorded itself are considered, and only after confirming
    the PID still belongs to an ffmpeg.exe — PIDs get reused, and killing a
    stranger's process would be far worse than leaving an orphan.

    `deadline` is a time.monotonic() instant shared by the whole cleanup sweep,
    so N orphans cost one ORPHAN_KILL_WAIT_MS between them rather than N."""
    try:
        with open(os.path.join(seg_dir, FFMPEG_PID_FILE)) as f:
            pid = int(f.read().strip())
    except Exception:
        return

    PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
    PROCESS_TERMINATE = 0x0001
    SYNCHRONIZE = 0x00100000        # required by WaitForSingleObject below
    k32 = ctypes.windll.kernel32
    handle = k32.OpenProcess(
        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | SYNCHRONIZE,
        False, pid)
    if not handle:
        return                      # already gone
    try:
        buf = ctypes.create_unicode_buffer(32768)
        size = wt.DWORD(len(buf))
        # PIDs get reused, so only kill it while it is still an ffmpeg.exe.
        if (k32.QueryFullProcessImageNameW(handle, 0, buf, ctypes.byref(size))
                and os.path.basename(buf.value).lower() == "ffmpeg.exe"):
            k32.TerminateProcess(handle, 1)
            # TerminateProcess is asynchronous: without waiting for the process
            # to actually exit, the caller's rmtree races its still-open segment
            # file and leaves the directory behind.
            wait_ms = ORPHAN_KILL_WAIT_MS
            if deadline is not None:
                # Floor, not max(0, ...): once a slow first orphan has eaten the
                # shared budget, a zero wait means the rmtree that follows races
                # the still-open segment file and the directory survives — the
                # very race this wait exists to close.
                wait_ms = max(ORPHAN_KILL_FLOOR_MS,
                              int((deadline - time.monotonic()) * 1000))
            k32.WaitForSingleObject(handle, wait_ms)
            log(f"killed orphan ffmpeg pid={pid} from {os.path.basename(seg_dir)}")
    finally:
        k32.CloseHandle(handle)


def main():
    # Single instance via Win32 mutex
    ctypes.windll.kernel32.CreateMutexW(None, True, "ClipRecorder_SingleInstance")
    if ctypes.windll.kernel32.GetLastError() == 183:  # ERROR_ALREADY_EXISTS
        sys.exit(0)

    # Clean up after a previous crash: kill the orphaned ffmpeg FIRST, then drop
    # its temp dir. Removing the directory alone is not enough — the process is
    # what holds an NVENC session.
    try:
        tmp = tempfile.gettempdir()
        deadline = time.monotonic() + ORPHAN_KILL_WAIT_MS / 1000.0
        for d in os.listdir(tmp):
            if not d.startswith(SEGMENT_DIR_PREFIX):
                continue
            path = os.path.join(tmp, d)
            # Caught per directory, not around the loop: orphans accumulate in
            # numbers (four were seen in practice), and the whole point of this
            # sweep is that ONE failure must not leave the rest holding their
            # NVENC sessions. Logged, because a silent skip here reproduces the
            # symptom this code exists to prevent.
            try:
                _kill_orphan_ffmpeg(path, deadline)
            except Exception as e:
                log(f"orphan cleanup failed for {d}: {type(e).__name__}: {e}")
            shutil.rmtree(path, ignore_errors=True)
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
        capture.save_replay(
            on_success=lambda secs: root.after(0, lambda: banner.show(secs)))

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
            desktop_lnk = desktop_lnk_path()
            # Leaving this one behind would make Windows try to launch a deleted
            # exe at every single sign-in, forever, with nothing left to fix it.
            startup_lnk = startup_lnk_path()
            pid = os.getpid()

            # Same rule as set_startup: paths go through the environment. This
            # is the sink that matters most — the helper is detached, hidden and
            # breaks away from the job object, so injected code would outlive
            # the app it was just told to remove.
            ps_script = (
                f"Wait-Process -Id {pid} -Timeout 10 -ErrorAction SilentlyContinue;"
                "Start-Sleep -Milliseconds 500;"
                "foreach ($v in 'CR_EXE','CR_CONFIG','CR_LOG','CR_DESKTOP_LNK','CR_STARTUP_LNK') {"
                "  Remove-Item -LiteralPath ([Environment]::GetEnvironmentVariable($v))"
                "    -Force -ErrorAction SilentlyContinue }"
            )
            uninstall_env = {**os.environ,
                             "CR_EXE": exe_path, "CR_CONFIG": CONFIG_FILE,
                             "CR_LOG": LOG_FILE, "CR_DESKTOP_LNK": desktop_lnk,
                             "CR_STARTUP_LNK": startup_lnk}
            # CREATE_BREAKAWAY_FROM_JOB is load-bearing: this helper waits for
            # us to exit before deleting the exe, so without it the kill-on-close
            # job takes it down with us and uninstall does nothing at all.
            # Guarded because breakaway is refused (ERROR_ACCESS_DENIED) inside
            # an outer job that forbids it — and an unhandled raise here would
            # skip root.destroy()/sys.exit() below, after capture, audio, hotkeys
            # and tray have already been torn down: a headless zombie process.
            try:
                subprocess.Popen(
                    ["powershell", "-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", ps_script],
                    creationflags=0x08000000 | CREATE_BREAKAWAY_FROM_JOB,
                    env=uninstall_env,
                )
            except OSError as e:
                log(f"uninstall helper could not break away: "
                    f"{type(e).__name__}: {e}")
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
