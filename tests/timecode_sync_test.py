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
    py tests\\timecode_sync_test.py --generate [--fps 60] [--buffer 15] [--monitor 1]
    py tests\\timecode_sync_test.py --analyze "C:\\path\\to\\Clip_....mp4"
"""
import argparse
import array
import importlib.util
import math
import os
import struct
import subprocess
import sys
import tempfile
import threading
import time
import wave

TESTS_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_DIR = os.path.dirname(TESTS_DIR)
APP_PATH = os.path.join(REPO_DIR, "src", "clip_recorder.pyw")

TONE_FREQ = 1000
TONE_RATE = 48000
BEEP_EVERY = 5.0          # a beep starts every N seconds of timecode
BEEP_LEN = 2.0            # and lasts this long
# Indicator block position/size as a fraction of the screen (top-left area).
IND_X, IND_Y, IND_W, IND_H = 0.02, 0.02, 0.20, 0.20

VIDEO_SAMPLE_FPS = 50     # 20 ms resolution on the indicator edges
AUDIO_WIN_MS = 10


def _ffmpeg_path():
    bundled = os.path.join(REPO_DIR, "ffmpeg.exe")
    return bundled if os.path.exists(bundled) else "ffmpeg"


def load_app_module():
    spec = importlib.util.spec_from_file_location("clip_recorder", APP_PATH)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


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
    tmp_wav = tempfile.mktemp(suffix=".wav")
    try:
        subprocess.run(
            [_ffmpeg_path(), "-y", "-i", mp4_path, "-ac", "1", "-ar", "48000", tmp_wav],
            capture_output=True, timeout=180, creationflags=0x08000000,
        )
        if not os.path.exists(tmp_wav):
            return None, 0
        with wave.open(tmp_wav, "rb") as wf:
            rate = wf.getframerate()
            raw = wf.readframes(wf.getnframes())
        return array.array("h", raw[:len(raw) - (len(raw) % 2)]), rate
    finally:
        try:
            os.remove(tmp_wav)
        except Exception:
            pass


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
    if errs:
        rc = 1

    expected, vdur = extract_indicator_intervals(mp4_path)
    samples, arate = _decode_audio(mp4_path)
    if samples is None:
        print("  ERROR: no audio track")
        return 1
    actual, adur = extract_audio_intervals(samples, arate)

    print(f"  video {vdur:.2f}s / audio {adur:.2f}s  (diff {abs(vdur - adur) * 1000:+.0f} ms)")
    if abs(vdur - adur) > 0.25:
        print("  DURATION MISMATCH")
        rc = 1

    clicks, per_s = count_discontinuities(samples, arate)
    print(f"  discontinuities: {clicks} ({per_s:.1f}/s)")
    if per_s > 1.0:
        print("  CRACKLING: abnormal sample-level steps (audio stitched with holes)")
        rc = 1

    print(f"  expected beeps (on-screen indicator): {len(expected)}")
    print(f"  actual beeps   (audio energy):        {len(actual)}")
    if not expected:
        print("  ERROR: indicator never detected — is this a timecode-test clip?")
        return 1
    if not actual:
        print("  ERROR: no audio bursts found — the sound is missing entirely")
        return 1

    print("  expected (video)      actual (audio)        offset")
    deltas = []
    for e0, e1 in expected:
        near = min(actual, key=lambda a: abs(a[0] - e0))
        if abs(near[0] - e0) > 2.5:
            print(f"    [{e0:6.2f}-{e1:6.2f}]   MISSING            --")
            rc = 1
            continue
        d = (near[0] - e0) * 1000
        deltas.append(d)
        print(f"    [{e0:6.2f}-{e1:6.2f}]   [{near[0]:6.2f}-{near[1]:6.2f}]   {d:+7.0f} ms")

    for a0, a1 in actual:
        if all(abs(a0 - e0) > 2.5 for e0, _ in expected):
            print(f"    (silence expected)    [{a0:6.2f}-{a1:6.2f}]   UNEXPECTED SOUND")
            rc = 1

    if save_tc is not None:
        report_recency(expected, vdur, save_tc)

    if deltas:
        deltas_sorted = sorted(deltas)
        mean = sum(deltas) / len(deltas)
        median = deltas_sorted[len(deltas_sorted) // 2]
        print(f"  offset mean {mean:+.0f} ms / median {median:+.0f} ms "
              f"(min {min(deltas):+.0f} / max {max(deltas):+.0f})")
        print("  (positive = audio LATE vs the picture; negative = audio EARLY/ahead)")
        if len(deltas) >= 2:
            drift = deltas[-1] - deltas[0]
            print(f"  drift first->last beep: {drift:+.0f} ms")
        if abs(median) > 300:
            print("  A/V MISALIGNED (median offset over 300 ms)")
            rc = 1
    return rc


# ─── Capture + signal ────────────────────────────────────────────────────────

def run_generate(args):
    cr = load_app_module()
    tk = cr.tk

    root = tk.Tk()
    root.withdraw()

    monitors = cr.get_monitors() or [{
        "name": "Default", "x": 0, "y": 0,
        "w": cr.user32.GetSystemMetrics(0), "h": cr.user32.GetSystemMetrics(1),
        "primary": True,
    }]
    mon_idx = min(args.monitor, len(monitors) - 1)
    mon = monitors[mon_idx]

    config = dict(cr.DEFAULTS)
    config["monitor"] = mon_idx
    config["fps"] = args.fps
    config["buffer_seconds"] = args.buffer

    audio = cr.AudioCapture(max_seconds=max(cr.BUFFER_OPTIONS + [args.buffer]))
    capture = cr.FFmpegCapture(root, config, monitors, audio)

    pa = cr.pyaudio.PyAudio()
    wasapi = pa.get_host_api_info_by_type(cr.pyaudio.paWASAPI)
    out_dev = pa.get_device_info_by_index(wasapi["defaultOutputDevice"])
    out_stream = pa.open(format=cr.pyaudio.paInt16, channels=2, rate=TONE_RATE,
                         output=True, output_device_index=out_dev["index"])
    beep = make_beep_pcm()

    win = tk.Toplevel(root)
    win.overrideredirect(True)
    win.attributes("-topmost", True)
    win.geometry(f"{mon['w']}x{mon['h']}+{mon['x']}+{mon['y']}")
    canvas = tk.Canvas(win, bg="black", highlightthickness=0)
    canvas.pack(fill="both", expand=True)
    ind = canvas.create_rectangle(
        mon["w"] * IND_X, mon["h"] * IND_Y,
        mon["w"] * (IND_X + IND_W), mon["h"] * (IND_Y + IND_H),
        fill="black", outline="")
    label = canvas.create_text(mon["w"] // 2, mon["h"] // 2, text="0.0",
                               fill="#20ff20", font=("Consolas", 140, "bold"))
    hint = canvas.create_text(mon["w"] // 2, mon["h"] // 2 + 160, text="",
                              fill="#808080", font=("Consolas", 40))
    win.deiconify()

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
        root.after(20, tick)

    def audio_loop():
        last_k = -1
        while not stop_flag.is_set():
            t = time.monotonic() - t0
            # One beep per scheduled window: the blocking write returns right at
            # the window's edge, so without this guard the loop re-triggers and
            # plays a 4 s beep instead of 2 s.
            if beep_expected(t) and int(t // BEEP_EVERY) != last_k:
                last_k = int(t // BEEP_EVERY)
                try:
                    out_stream.write(beep)      # blocks ~BEEP_LEN, exactly the window
                except Exception:
                    pass
            else:
                time.sleep(0.01)                # write NOTHING: real digital silence

    capture.start()
    print(f"Capturing {args.fps} FPS, buffer={args.buffer}s, monitor={mon_idx}")
    print(f"Schedule decided in advance: beep every {BEEP_EVERY:g}s for {BEEP_LEN:g}s")
    print("  -> sound expected at timecode [5,7) [10,12) [15,17) ... ; silence elsewhere")
    tick()
    threading.Thread(target=audio_loop, daemon=True).start()

    result = {}

    def on_success():
        folder = cr.get_output_folder(config)
        files = [os.path.join(folder, f) for f in os.listdir(folder) if f.startswith("Clip_")]
        result["path"] = max(files, key=os.path.getmtime) if files else None
        print(f"  saved: {result['path']}")
        stop_flag.set()
        root.after(400, root.quit)

    def do_save():
        result["save_tc"] = time.monotonic() - t0
        print(f"Triggering save_replay() at timecode {result['save_tc']:.1f}s ...")
        capture.save_replay(on_success=lambda: root.after(0, on_success))

    # --save-after forces a save BEFORE the buffer has refilled, which is what
    # happens right after a settings change: the selection falls back to "take
    # everything", a different anchoring path.
    save_at = args.save_after if args.save_after else args.buffer + 12
    root.after(int(save_at * 1000), do_save)
    root.after(int((save_at + 45) * 1000), root.quit)   # hard safety stop
    root.mainloop()

    stop_flag.set()
    for fn in (lambda: out_stream.stop_stream(), lambda: out_stream.close(),
               lambda: pa.terminate(), lambda: win.destroy()):
        try:
            fn()
        except Exception:
            pass
    capture.cleanup()
    audio.cleanup()

    path = result.get("path")
    if not path:
        print("ERROR: save_replay never completed")
        return 1
    print()
    return run_analyze(path, save_tc=result.get("save_tc"))


def main():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    m = p.add_mutually_exclusive_group(required=True)
    m.add_argument("--generate", action="store_true")
    m.add_argument("--analyze", metavar="MP4_PATH")
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
    args = p.parse_args()
    sys.exit(run_analyze(args.analyze) if args.analyze else run_generate(args))


if __name__ == "__main__":
    main()
