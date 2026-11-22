"""
Timecode + scheduled-beep A/V sync test for Clip Recorder.

Ground truth is decided IN ADVANCE: a running timecode is drawn full-screen on
the capture monitor, and a beep of BEEP_LEN seconds is played every BEEP_EVERY
seconds (so sound is expected during [5,7), [10,12), ... and NOT expected in
between). A large indicator block on screen is WHITE for exactly the scheduled
beep windows and black otherwise.

Because the indicator lives in the video track, the analyzer can recover
"sound was supposed to be playing here" straight from the picture and compare it
against the real audio energy — measuring the A/V offset intrinsically, with no
OCR and no assumption about where the clip starts. It also proves the silent
stretches really are silent (the WASAPI-loopback-drops-silence case) and flags
sample-level discontinuities (crackling).

Usage:
    py tests\\timecode_sync_test.py --generate [--fps 60] [--buffer 15] [--monitor 1] [--exe path]
    py tests\\timecode_sync_test.py --analyze "C:\\path\\to\\Clip_....mp4"
"""
import argparse
import array
import collections
import math
import os
import struct
import subprocess
import sys
import threading
import time

TESTS_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_DIR = os.path.dirname(TESTS_DIR)

TONE_FREQ = 1000
TONE_RATE = 48000
BEEP_EVERY = 5.0          # a beep starts every N seconds of timecode
BEEP_LEN = 2.0            # and lasts this long
# Indicator block position/size as a fraction of the screen (top-left area).
IND_X, IND_Y, IND_W, IND_H = 0.02, 0.02, 0.20, 0.20

# What an analysis measured, written by --json for scripts/release-check.ps1.
RESULT = {}

VIDEO_SAMPLE_FPS = 50     # 20 ms resolution on the indicator edges
# Live meter: what Windows really plays, read back through a WASAPI loopback.
METER_FLOOR_DB = -60.0
LEAK_DB = -50.0           # the test beep plays at -20 dBFS; digital silence reads nothing
MIC_LEAK_DB = -30.0       # a mic always hisses (about -60 to -45 dBFS): only a voice or a knock counts
LEAK_MARGIN = 0.3         # s around each beep's edges: output latency, fades
MIC_MARGIN = 0.6          # the mic hears the beep through the air, after the speaker's latency
LEAK_GAP = 0.5            # s: sounds closer than this are one leak (one sentence)
METER_STALE = 0.1         # s: WASAPI loopback delivers nothing during digital silence
AUDIO_WIN_MS = 10


def _ffmpeg_path():
    bundled = os.path.join(REPO_DIR, "target", "ffmpeg", "ffmpeg.exe")
    return bundled if os.path.exists(bundled) else "ffmpeg"


def beep_expected(t):
    """Ground truth: is a beep scheduled at timecode t (seconds)?"""
    if t < BEEP_EVERY:
        return False
    return (t % BEEP_EVERY) < BEEP_LEN


def make_beep_pcm(seconds=BEEP_LEN, rate=TONE_RATE, freq=TONE_FREQ, channels=2):
    """A clean tone with short fades, so the SIGNAL itself has no clicks —
    otherwise the discontinuity check would flag our own test tone."""
    n = int(rate * seconds)
    fade = max(1, int(rate * 0.01))
    samples = []
    for i in range(n):
        env = min(i, n - i, fade) / fade
        val = int(32767 * 0.10 * env * math.sin(2 * math.pi * freq * i / rate))
        samples.extend([val] * channels)
    return struct.pack("<" + "h" * len(samples), *samples)


def peak_dbfs(samples):
    """Peak of int16 samples in dBFS; -inf for silence."""
    peak = max((abs(v) for v in samples), default=0)
    return 20 * math.log10(peak / 32768) if peak else float("-inf")


def drain_loudest(blocks):
    """The loudest of the peaks queued since the last read, emptying the queue; None if none."""
    loudest = None
    while blocks:
        db = blocks.popleft()
        loudest = db if loudest is None else max(loudest, db)
    return loudest


def in_silence(t, margin=LEAK_MARGIN):
    """True when no beep is scheduled at timecode t, nor within `margin` of one."""
    k = round(t / BEEP_EVERY)
    for start in (k * BEEP_EVERY, (k - 1) * BEEP_EVERY):
        if start >= BEEP_EVERY and start - margin <= t <= start + BEEP_LEN + margin:
            return False
    return True


class LeakCounter:
    """Counts the sounds heard during scheduled silence: a rise above `threshold` is one, unless
    it follows the last one by under LEAK_GAP (the syllables of one sentence)."""

    def __init__(self, threshold=LEAK_DB, margin=LEAK_MARGIN):
        self.threshold = threshold
        self.margin = margin
        self.count = 0
        self._on = False
        self._heard_at = float("-inf")

    def update(self, t, db):
        on = in_silence(t, self.margin) and db > self.threshold
        if on and not self._on and t - self._heard_at > LEAK_GAP:
            self.count += 1
        if on:
            self._heard_at = t
        self._on = on
        return self.count


# ─── Analysis ────────────────────────────────────────────────────────────────

def _intervals_from_flags(flags, dt, min_len=0.15):
    """[(start, end)] for each run of True in `flags`, sampled every `dt` s."""
    out = []
    start = None
    for i, on in enumerate(flags):
        if on and start is None:
            start = i
        elif not on and start is not None:
            if (i - start) * dt >= min_len:
                out.append((start * dt, i * dt))
            start = None
    if start is not None and (len(flags) - start) * dt >= min_len:
        out.append((start * dt, len(flags) * dt))
    return out


def extract_indicator_intervals(mp4_path):
    """When the on-screen indicator is lit = when sound was SUPPOSED to play."""
    proc = subprocess.run(
        [_ffmpeg_path(), "-y", "-i", mp4_path,
         "-vf", (f"fps={VIDEO_SAMPLE_FPS},"
                 f"crop=iw*{IND_W}:ih*{IND_H}:iw*{IND_X}:ih*{IND_Y},"
                 f"scale=1:1,format=gray"),
         "-f", "rawvideo", "-pix_fmt", "gray", "-"],
        capture_output=True, timeout=180, creationflags=0x08000000,
    )
    vals = list(proc.stdout)
    if not vals:
        return [], 0.0
    lo, hi = min(vals), max(vals)
    if hi - lo < 40:                      # never lit → no usable signal
        return [], len(vals) / VIDEO_SAMPLE_FPS
    thr = (lo + hi) / 2
    flags = [v > thr for v in vals]
    return (_intervals_from_flags(flags, 1.0 / VIDEO_SAMPLE_FPS),
            len(vals) / VIDEO_SAMPLE_FPS)


def _decode_audio(mp4_path):
    rate = 48000
    result = subprocess.run(
        [_ffmpeg_path(), "-i", mp4_path, "-ac", "1", "-ar", str(rate),
         "-f", "s16le", "-"],
        capture_output=True, timeout=180, creationflags=0x08000000,
    )
    raw = result.stdout      # analysed even on a non-zero exit, as before
    if not raw:
        return None, 0
    return array.array("h", raw[:len(raw) - (len(raw) % 2)]), rate


def extract_audio_intervals(samples, rate):
    """When sound is ACTUALLY present."""
    win = max(1, int(rate * AUDIO_WIN_MS / 1000))
    energies = []
    for i in range(0, len(samples) - win, win):
        chunk = samples[i:i + win]
        energies.append(math.sqrt(sum(s * s for s in chunk) / len(chunk)))
    if not energies:
        return [], 0.0
    peak = max(energies)
    if peak < 200:                        # essentially silent track
        return [], len(samples) / rate
    thr = peak * 0.25
    flags = [e >= thr for e in energies]
    return (_intervals_from_flags(flags, win / rate),
            len(samples) / rate)


def count_discontinuities(samples, rate):
    """Sample-to-sample steps far above the local norm = clicks/crackle.
    Returns (count, per_second)."""
    if len(samples) < 3:
        return 0, 0.0
    diffs = [abs(samples[i] - samples[i - 1]) for i in range(1, len(samples))]
    nz = [d for d in diffs if d > 0]
    if not nz:
        return 0, 0.0
    nz.sort()
    p95 = nz[min(len(nz) - 1, int(len(nz) * 0.95))]
    # A 1 kHz tone's natural step is ~2pi*f/rate * amplitude; a hole punched into
    # the waveform steps straight to/from zero, i.e. several times larger. Keying
    # off the 95th percentile keeps this sensitive at any tone amplitude
    # (a median*100 threshold could never be reached — int16 caps at 32767).
    thr = max(1200, p95 * 4)
    count = sum(1 for d in diffs if d >= thr)
    dur = len(samples) / rate
    return count, (count / dur if dur else 0.0)


def match_beeps(expected, actual, end=None):
    """Pairs each expected beep (video) with the audio burst that starts nearest, None when no
    burst starts within 2.5 s; and the bursts that match no expected beep. A beep cut by the
    clip measures nothing, and its sound can be too short to detect: left out on both sides.
    That is a picture interval from the first sample, a sound burst in the first 0.25 s when no
    kept beep starts there (beeps are 5 s apart), and a short fragment ending at `end`."""
    def tail(iv):
        return end is not None and iv[1] >= end - 0.03 and iv[1] - iv[0] < 0.3

    expected = [iv for iv in expected if iv[0] > 0 and not tail(iv)]
    early = any(e0 < 0.25 for e0, _ in expected)
    actual = [iv for iv in actual if (iv[0] >= 0.25 or early) and not tail(iv)]
    pairs = []
    for e0, e1 in expected:
        near = min(actual, key=lambda a: abs(a[0] - e0)) if actual else None
        pairs.append(((e0, e1), near if near and abs(near[0] - e0) <= 2.5 else None))
    unexpected = [a for a in actual if all(abs(a[0] - e0) > 2.5 for e0, _ in expected)]
    return pairs, unexpected


def report_recency(expected, vdur, save_tc):
    """How much of the moment right before the hotkey is missing from the clip.
    Each on-screen beep starts at a known timecode (k*BEEP_EVERY), so one
    detected beep pins the clip to the timecode axis."""
    cand = [iv for iv in expected if iv[0] > 0.05] or expected
    if not cand:
        return
    c = cand[0][0]
    # The schedule is periodic, so pinning is only unique modulo BEEP_EVERY:
    # every k gives a candidate loss differing by one period. The physical loss
    # is bounded (it is just the age of the newest COMPLETE segment), so take
    # the smallest non-negative candidate rather than the first one found.
    losses = []
    for k in range(1, 500):
        start_tc = k * BEEP_EVERY - c
        loss = save_tc - (start_tc + vdur)
        if loss >= -0.5:
            losses.append((loss, start_tc))
    if not losses:
        print("  (could not pin the clip to the timecode axis)")
        return
    loss, start_tc = min(losses, key=lambda x: abs(x[0]))
    RESULT["lost_before_press_s"] = round(loss, 3)
    print(f"  clip covers timecode {start_tc:.2f}s -> {start_tc + vdur:.2f}s; "
          f"hotkey pressed at {save_tc:.2f}s")
    print(f"  LOST BEFORE PRESS: {loss:.2f}s "
          f"({'good' if loss < 1.5 else 'TOO MUCH — the moment is cut off'})"
          f"   [+-{BEEP_EVERY:g}s ambiguity: periodic schedule]")


def run_analyze(mp4_path, save_tc=None):
    print(f"Analyzing {mp4_path}")
    print(f"  schedule: beep every {BEEP_EVERY:g}s lasting {BEEP_LEN:g}s")
    rc = 0

    errs = subprocess.run(
        [_ffmpeg_path(), "-v", "error", "-i", mp4_path, "-f", "null", "-"],
        capture_output=True, timeout=180, creationflags=0x08000000,
    ).stderr.decode("utf-8", "replace").strip()
    print(f"  decode: {'clean' if not errs else 'ERRORS: ' + errs[:200]}")
    RESULT["decode_clean"] = not errs
    if errs:
        rc = 1

    expected, vdur = extract_indicator_intervals(mp4_path)
    samples, arate = _decode_audio(mp4_path)
    if samples is None:
        print("  ERROR: no audio track")
        return 1
    actual, adur = extract_audio_intervals(samples, arate)

    print(f"  video {vdur:.2f}s / audio {adur:.2f}s  (diff {abs(vdur - adur) * 1000:+.0f} ms)")
    RESULT["duration_diff_ms"] = round(abs(vdur - adur) * 1000)
    if abs(vdur - adur) > 0.25:
        print("  DURATION MISMATCH")
        rc = 1

    clicks, per_s = count_discontinuities(samples, arate)
    print(f"  discontinuities: {clicks} ({per_s:.1f}/s)")
    RESULT["discontinuities"] = clicks
    if per_s > 1.0:
        print("  CRACKLING: abnormal sample-level steps (audio stitched with holes)")
        rc = 1

    print(f"  expected beeps (on-screen indicator): {len(expected)}")
    print(f"  actual beeps   (audio energy):        {len(actual)}")
    RESULT["beeps_expected"] = len(expected)
    if not expected:
        print("  ERROR: indicator never detected — is this a timecode-test clip?")
        return 1
    if not actual:
        print("  ERROR: no audio bursts found — the sound is missing entirely")
        return 1

    print("  expected (video)      actual (audio)        offset")
    pairs, unexpected = match_beeps(expected, actual, end=vdur)
    deltas = []
    for (e0, e1), near in pairs:
        if near is None:
            print(f"    [{e0:6.2f}-{e1:6.2f}]   MISSING            --")
            RESULT["beeps_missing"] = RESULT.get("beeps_missing", 0) + 1
            rc = 1
            continue
        d = (near[0] - e0) * 1000
        deltas.append(d)
        print(f"    [{e0:6.2f}-{e1:6.2f}]   [{near[0]:6.2f}-{near[1]:6.2f}]   {d:+7.0f} ms")

    for a0, a1 in unexpected:
        print(f"    (silence expected)    [{a0:6.2f}-{a1:6.2f}]   UNEXPECTED SOUND")
        RESULT["unexpected_sound"] = RESULT.get("unexpected_sound", 0) + 1
        rc = 1

    if save_tc is not None:
        report_recency(expected, vdur, save_tc)

    if not deltas:
        print("  ERROR: no beep could be measured")
        rc = 1
    if deltas:
        deltas_sorted = sorted(deltas)
        mean = sum(deltas) / len(deltas)
        median = deltas_sorted[len(deltas_sorted) // 2]
        RESULT["median_offset_ms"] = round(median)
        RESULT["max_abs_offset_ms"] = round(max(abs(d) for d in deltas))
        print(f"  offset mean {mean:+.0f} ms / median {median:+.0f} ms "
              f"(min {min(deltas):+.0f} / max {max(deltas):+.0f})")
        print("  (positive = audio LATE vs the picture; negative = audio EARLY/ahead)")
        if len(deltas) >= 2:
            drift = deltas[-1] - deltas[0]
            print(f"  drift first->last beep: {drift:+.0f} ms")
        if abs(median) > 300:
            print("  A/V MISALIGNED (median offset over 300 ms)")
            rc = 1
    RESULT["rc"] = rc
    return rc


# ─── Capture + signal ────────────────────────────────────────────────────────

def monitors_primary_first():
    """(x, y, w, h, primary) per screen, primary first: the app's own order."""
    import ctypes
    from ctypes import wintypes
    user32 = ctypes.windll.user32
    found = []

    class MONITORINFO(ctypes.Structure):
        _fields_ = [("cbSize", wintypes.DWORD), ("rcMonitor", wintypes.RECT),
                    ("rcWork", wintypes.RECT), ("dwFlags", wintypes.DWORD)]

    proc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HMONITOR, wintypes.HDC,
                              ctypes.POINTER(wintypes.RECT), wintypes.LPARAM)

    def visit(handle, _dc, _rect, _data):
        info = MONITORINFO(cbSize=ctypes.sizeof(MONITORINFO))
        user32.GetMonitorInfoW(handle, ctypes.byref(info))
        r = info.rcMonitor
        found.append((r.left, r.top, r.right - r.left, r.bottom - r.top, bool(info.dwFlags & 1)))
        return True

    user32.SetProcessDPIAware()
    user32.EnumDisplayMonitors(None, None, proc(visit), 0)
    return sorted(found, key=lambda m: not m[4])


def press_hotkey():
    """Ctrl+Alt+Shift+F12: the harness's own hotkey, so an installed Clip Recorder holding
    Ctrl+Alt+R is left alone."""
    import ctypes
    keys = (0x11, 0x12, 0x10, 0x7B)
    for k in keys:
        ctypes.windll.user32.keybd_event(k, 0, 0, 0)
    for k in reversed(keys):
        ctypes.windll.user32.keybd_event(k, 0, 2, 0)


def run_generate(args):
    """Drives the real app (the Rust exe, on its own data folder) through its real hotkey."""
    import json
    import shutil
    import tempfile
    import tkinter as tk
    import pyaudiowpatch as pyaudio

    screens = monitors_primary_first()
    mon_idx = min(args.monitor, len(screens) - 1)
    mx, my, mw, mh, _ = screens[mon_idx]

    data = os.path.join(tempfile.gettempdir(), f"crsync-{os.getpid()}")
    clips = os.path.join(data, "clips")
    shutil.rmtree(data, ignore_errors=True)
    os.makedirs(clips)
    with open(os.path.join(data, "config.json"), "w", encoding="utf-8") as f:
        json.dump({"monitor": mon_idx, "fps": args.fps, "buffer_seconds": args.buffer,
                   "output_folder": clips, "hotkey": "Ctrl+Alt+Shift+F12", "format": args.format}, f)
    env = dict(os.environ, CLIPREC_DATA_DIR=data, CLIPREC_FFMPEG=_ffmpeg_path())
    app = subprocess.Popen([args.exe], env=env)

    pa = pyaudio.PyAudio()
    wasapi = pa.get_host_api_info_by_type(pyaudio.paWASAPI)
    out_dev = pa.get_device_info_by_index(wasapi["defaultOutputDevice"])
    # WASAPI shared mode demands the endpoint's OWN rate: a 44.1 kHz output (any Bluetooth
    # speaker) made a fixed 48 kHz open raise -9997 Invalid sample rate.
    tone_rate = int(out_dev["defaultSampleRate"])
    out_stream = pa.open(format=pyaudio.paInt16, channels=2, rate=tone_rate,
                         output=True, output_device_index=out_dev["index"])
    beep = make_beep_pcm(rate=tone_rate)

    root = tk.Tk()
    root.overrideredirect(True)
    root.attributes("-topmost", True)
    root.geometry(f"{mw}x{mh}+{mx}+{my}")
    canvas = tk.Canvas(root, bg="black", highlightthickness=0)
    canvas.pack(fill="both", expand=True)
    ind = canvas.create_rectangle(mw * IND_X, mh * IND_Y, mw * (IND_X + IND_W), mh * (IND_Y + IND_H),
                                  fill="black", outline="")
    label = canvas.create_text(mw // 2, mh // 2, text="0.0", fill="#20ff20", font=("Consolas", 140, "bold"))
    hint = canvas.create_text(mw // 2, mh // 2 + 160, text="", fill="#808080", font=("Consolas", 40))
    # Live meters at the bottom, far from the indicator the analysis crops (top-left): what Windows
    # plays (Discord, Spotify, a notification) and the mic, both mixed into the clip. Each callback
    # is the only writer of its `level`, one tuple swapped at a time.
    meters = []

    def add_meter(name, find_device, threshold, margin, y):
        blocks = collections.deque(maxlen=500)

        def on_audio(data, _frames, _time, _status):
            blocks.append(peak_dbfs(array.array("h", data)))
            return None, pyaudio.paContinue

        try:    # a meter that will not open (mic privacy, exclusive mode) is no meter, not a crash
            device = find_device()
            rate = int(device["defaultSampleRate"])
            stream = pa.open(format=pyaudio.paInt16, channels=device["maxInputChannels"], rate=rate, input=True,
                             input_device_index=device["index"], frames_per_buffer=rate // 50,
                             stream_callback=on_audio)
        except (OSError, LookupError) as e:
            print(f"  no {name} meter: {e}")
            return
        x0, x1 = mw * 0.25, mw * 0.75
        canvas.create_text(x0 - 20, y + 18, text=name, fill="#c0c0c0", anchor="e", font=("Consolas", 28))
        canvas.create_rectangle(x0, y, x1, y + 36, fill="#202020", outline="#404040")
        meters.append({"name": name, "blocks": blocks, "heard": (0.0, float("-inf")), "stream": stream,
                       "leaks": LeakCounter(threshold, margin),
                       "x": (x0, x1, y), "bar": canvas.create_rectangle(x0, y, x0, y + 36, outline=""),
                       "text": canvas.create_text(x1 + 20, y + 18, fill="#c0c0c0", anchor="w",
                                                  font=("Consolas", 28))})

    add_meter("OUT", pa.get_default_wasapi_loopback, LEAK_DB, LEAK_MARGIN, mh * 0.80)
    add_meter("MIC", lambda: pa.get_device_info_by_index(wasapi["defaultInputDevice"]), MIC_LEAK_DB, MIC_MARGIN,
              mh * 0.80 + 56)
    leak_text = canvas.create_text(mw // 2, mh * 0.80 - 40, text="", fill="#ff3030",
                                   font=("Consolas", 36, "bold"))

    t0 = time.monotonic()
    stop_flag = threading.Event()

    def tick():
        if stop_flag.is_set():
            return
        t = time.monotonic() - t0
        on = beep_expected(t)
        canvas.itemconfig(ind, fill=("white" if on else "black"))
        canvas.itemconfig(label, text=f"{t:6.1f}")
        canvas.itemconfig(hint, text="BEEP" if on else "silence")
        for m in meters:
            loudest = drain_loudest(m["blocks"])
            if loudest is not None:
                m["heard"] = (time.monotonic(), loudest)
            heard_at, db = m["heard"]
            if time.monotonic() - heard_at > METER_STALE:
                db = float("-inf")
            x0, x1, y = m["x"]
            fill = max(0.0, min(1.0, (db - METER_FLOOR_DB) / -METER_FLOOR_DB))
            canvas.coords(m["bar"], x0, y, x0 + (x1 - x0) * fill, y + 36)
            canvas.itemconfig(m["bar"], fill="#ff3030" if db > -1 else "#ff9020" if db > -6 else "#20c020")
            canvas.itemconfig(m["text"], text="silence" if db == float("-inf") else f"{db:6.1f} dBFS")
            if "save_tc" not in result:     # the app's own save chime comes after the press
                m["leaks"].update(t, db)
                if in_silence(t, m["leaks"].margin):
                    m["loudest_silent"] = max(m.get("loudest_silent", float("-inf")), db)
        heard = [f"{m['name']} x{m['leaks'].count}" for m in meters if m["leaks"].count]
        canvas.itemconfig(leak_text, text=("SOUND LEAK  " + "  ".join(heard)) if heard else "")
        root.after(20, tick)

    def audio_loop():
        last_k = -1
        while not stop_flag.is_set():
            t = time.monotonic() - t0
            # One beep per window: the blocking write returns right at the window's edge, so
            # without this guard the loop re-triggers and plays a 4 s beep.
            if beep_expected(t) and int(t // BEEP_EVERY) != last_k:
                last_k = int(t // BEEP_EVERY)
                try:
                    out_stream.write(beep)      # blocks ~BEEP_LEN, exactly the window
                except Exception:
                    pass
            else:
                time.sleep(0.01)                # write NOTHING: real digital silence

    print(f"Capturing {args.fps} FPS, buffer={args.buffer}s, monitor={mon_idx} with {args.exe}")
    print(f"Schedule decided in advance: beep every {BEEP_EVERY:g}s for {BEEP_LEN:g}s")
    result = {}
    tick()
    threading.Thread(target=audio_loop, daemon=True).start()

    def wait_for_clip(deadline):
        files = [f for f in os.listdir(clips) if f.endswith("." + args.format)]
        if files:
            result["path"] = os.path.join(clips, files[0])
            print(f"  saved: {result['path']}")
            root.after(1500, root.quit)     # let the mux finish writing
        elif time.monotonic() < deadline:
            root.after(200, wait_for_clip, deadline)
        else:
            root.quit()

    def do_save():
        result["save_tc"] = time.monotonic() - t0
        print(f"Pressing the hotkey at timecode {result['save_tc']:.1f}s ...")
        press_hotkey()
        wait_for_clip(time.monotonic() + 45)

    # --save-after forces a save BEFORE the buffer has refilled (the take-everything path).
    save_at = args.save_after if args.save_after else args.buffer + 12
    root.after(int(save_at * 1000), do_save)
    root.mainloop()

    stop_flag.set()
    streams = [out_stream] + [m["stream"] for m in meters]
    for fn in [f for st in streams for f in (st.stop_stream, st.close)] + [pa.terminate, root.destroy, app.kill]:
        try:
            fn()
        except Exception:
            pass
    app.wait()

    path = result.get("path")
    if not path:
        print(f"ERROR: no clip was saved; see {os.path.join(data, 'clip_recorder.log')}")
        return 1
    for m in meters:
        print(f"  {m['name']}: loudest during scheduled silence {m.get('loudest_silent', float('-inf')):.1f} dBFS "
              f"(leak above {m['leaks'].threshold:g})")
        RESULT["sound_leaks" if m["name"] == "OUT" else "mic_leaks"] = m["leaks"].count
        if m["leaks"].count:
            print(f"SOUND LEAK ({m['name']}): {m['leaks'].count} sound(s) during scheduled silence; "
                  "this run is polluted")
    print()
    rc = run_analyze(path, save_tc=result.get("save_tc"))
    if any(m["leaks"].count for m in meters):
        rc = RESULT["rc"] = 1      # a polluted run measured nothing worth trusting
    return rc


def self_check():
    """The analysis's pure parts, checked without audio, a window or the app (tests/source.rs)."""
    # 120 fps A/B, 2026-11-22: the clip opened on a beep's last 0.18 s of picture; its 0.14 s of
    # sound was under min_len, so the first beep read MISSING and the verdict failed.
    expected = [(0.00, 0.18), (3.18, 5.20), (8.16, 10.18), (13.18, 15.00)]
    actual = [(3.14, 5.14), (8.14, 10.14), (13.15, 14.99)]
    pairs, unexpected = match_beeps(expected, actual)
    assert all(near is not None for _, near in pairs), pairs
    assert not unexpected, unexpected
    # The same fragment, when detected, is the clip's cut on both sides: no free "+0 ms".
    pairs, unexpected = match_beeps(expected, [(0.00, 0.16)] + actual)
    assert [round((n[0] - e[0]) * 1000) for e, n in pairs] == [-40, -20, -30], pairs
    assert not unexpected, unexpected
    # A real onset right after the cut is measured, its sound early or late.
    for beep, sound, offset in [((0.06, 2.06), (0.00, 2.03), -60), ((0.00, 2.04), (0.03, 2.07), None)]:
        pairs, unexpected = match_beeps([beep, (5.06, 7.06)], [sound, (5.03, 7.03)])
        assert [round((n[0] - e[0]) * 1000) for e, n in pairs][:1] == ([offset] if offset else [-30]), pairs
        assert not unexpected, unexpected
    # The clip's end can cut a beep too: 0.17 s of picture, 0.14 s of sound (under min_len).
    pairs, unexpected = match_beeps([(3.20, 5.20), (14.83, 15.00)], [(3.18, 5.18)], end=15.0)
    assert all(near is not None for _, near in pairs) and not unexpected, pairs
    # ...or its sound only; a whole last beep running into the end is still measured.
    pairs, unexpected = match_beeps([(3.20, 5.20), (13.18, 15.00)],
                                    [(3.18, 5.18), (13.15, 14.99), (14.86, 15.00)], end=15.0)
    assert len(pairs) == 2 and all(near is not None for _, near in pairs) and not unexpected, (pairs, unexpected)
    pairs, unexpected = match_beeps([(3.20, 5.20)], [(3.18, 5.18), (14.84, 15.00)], end=15.0)
    assert not unexpected, unexpected       # 0.16 s of sound, its 0.14 s of picture undetected
    # The meter: the test beep (10 % of full scale) reads -20 dBFS, digital silence -inf.
    assert round(peak_dbfs([0, 3277, -3277])) == -20
    assert peak_dbfs([]) == peak_dbfs([0, 0]) == float("-inf")
    # Every block between two screen refreshes counts, not only the last one.
    blocks = collections.deque([-60.0, -12.0, -70.0])
    assert drain_loudest(blocks) == -12.0 and not blocks and drain_loudest(blocks) is None
    # Silence: before the first beep, and between beeps away from their edges only.
    expected_silence = [True, True, False, False, False, True, True, False]
    assert [in_silence(t) for t in (1.0, 4.6, 4.8, 6.0, 7.2, 7.4, 9.6, 9.8)] == expected_silence
    # The mic also hears the beep through the air, late (Bluetooth speakers): a wider margin.
    assert in_silence(7.5) and not in_silence(7.5, MIC_MARGIN) and in_silence(8.0, MIC_MARGIN)
    leaks = LeakCounter()
    for t, db in [(1.0, -30), (1.1, -30), (1.2, -90), (6.0, -20), (8.0, -45), (8.1, -45)]:
        leaks.update(t, db)
    assert leaks.count == 2, leaks.count     # two rises in silence; the beep at 6.0 is not one
    mic = LeakCounter(MIC_LEAK_DB, MIC_MARGIN)
    for t, db in [(1.0, -45), (1.5, -52), (2.0, -18), (2.1, -48), (6.0, -12)]:
        mic.update(t, db)
    assert mic.count == 1, mic.count         # the hiss is not a leak, the voice at 2.0 is
    # One sentence is one leak, whatever its syllables (27 counted for a few words, 2026-11-22);
    # a second sentence a second later is another.
    for t, db in [(2.3, -20), (2.4, -50), (2.6, -15), (2.8, -50), (3.0, -22), (3.2, -50), (4.0, -20)]:
        mic.update(t, db)
    assert mic.count == 2, mic.count
    mic.update(4.2, -50)
    mic.update(7.5, -15)                     # the beep's echo, 0.5 s after it ends: not a leak
    assert mic.count == 2, mic.count
    print("self-check: ok")
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    m = p.add_mutually_exclusive_group(required=True)
    m.add_argument("--generate", action="store_true")
    m.add_argument("--analyze", metavar="MP4_PATH")
    m.add_argument("--self-check", action="store_true", help="Check the analysis's pure parts and exit.")
    p.add_argument("--exe", default=os.path.join(REPO_DIR, "target", "release", "clip-recorder.exe"),
                   help="The app to drive, on its own data folder (an installed one is untouched).")
    p.add_argument("--fps", type=int, default=60)
    p.add_argument("--buffer", type=int, default=15)
    p.add_argument("--monitor", type=int, default=1,
                   help="Capture/display monitor. Defaults to the SECOND monitor: a "
                        "full-screen signal on the primary would take over the "
                        "user's main display (clamped if only one exists).")
    p.add_argument("--save-after", type=float, default=None, metavar="SEC",
                   help="Save at this timecode instead of waiting for the buffer to "
                        "fill — exercises the not-enough-history path taken right "
                        "after a settings change.")
    p.add_argument("--format", choices=["mp4", "mkv"], default="mp4",
                   help="The clip container the app writes (its \"format\" setting).")
    p.add_argument("--json", metavar="FILE", default=None,
                   help="Also write what the analysis measured to FILE (read by scripts/release-check.ps1).")
    args = p.parse_args()
    if args.self_check:
        sys.exit(self_check())
    rc = run_analyze(args.analyze) if args.analyze else run_generate(args)
    if args.json:
        import json
        RESULT.setdefault("rc", rc)
        with open(args.json, "w", encoding="utf-8") as f:
            json.dump(RESULT, f, indent=1)
    sys.exit(rc)


if __name__ == "__main__":
    main()
