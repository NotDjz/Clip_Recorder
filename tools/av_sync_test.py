"""
Automated audio/video sync test for Clip Recorder.

Shows a full-screen color-flip signal on the configured capture monitor,
plays a synchronized tone through the default audio output device (so
WASAPI loopback captures it) on the same flip cadence, saves a real replay
via FFmpegCapture.save_replay(), then measures the actual offset between
video color transitions and audio tone onsets in the output clip.

Usage:
    py tools\\av_sync_test.py --generate [--fps 240] [--buffer 120] [--monitor 0]
    py tools\\av_sync_test.py --analyze "C:\\path\\to\\Clip_20260101_000000.mp4"
"""
import argparse
import importlib.util
import math
import os
import re
import struct
import subprocess
import sys
import tempfile
import threading
import time
import wave

TOOLS_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_DIR = os.path.dirname(TOOLS_DIR)
APP_PATH = os.path.join(REPO_DIR, "clip_recorder.pyw")

TONE_FREQ = 1000
TONE_MS = 100
TONE_RATE = 48000
FLIP_INTERVAL = 1.0


def _ffmpeg_path():
    bundled = os.path.join(REPO_DIR, "ffmpeg.exe")
    return bundled if os.path.exists(bundled) else "ffmpeg"


def _decode_errors(mp4_path):
    """Return decoder error text ('' = clean). Catches the 'corrupt decoded
    frame' class from muxing a still-being-written segment."""
    proc = subprocess.run(
        [_ffmpeg_path(), "-v", "error", "-i", mp4_path, "-f", "null", "-"],
        capture_output=True, timeout=120, creationflags=0x08000000,
    )
    return proc.stderr.decode("utf-8", "replace").strip()


def _stream_duration(mp4_path, selector):
    """Decode one stream (e.g. '0:v:0' / '0:a:0') to null and return its
    duration in seconds, from ffmpeg's final 'time=' progress token."""
    proc = subprocess.run(
        [_ffmpeg_path(), "-i", mp4_path, "-map", selector, "-f", "null", "-"],
        capture_output=True, timeout=120, creationflags=0x08000000,
    )
    err = proc.stderr.decode("utf-8", "replace")
    last = None
    for m in re.finditer(r"time=(\d+):(\d+):(\d+(?:\.\d+)?)", err):
        h, mm, ss = m.groups()
        last = int(h) * 3600 + int(mm) * 60 + float(ss)
    return last


def load_app_module():
    spec = importlib.util.spec_from_file_location("clip_recorder", APP_PATH)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def make_tone_pcm(rate=TONE_RATE, freq=TONE_FREQ, ms=TONE_MS, channels=2):
    n = int(rate * ms / 1000)
    fade = max(1, rate // 100)
    samples = []
    for i in range(n):
        env = min(i, n - i, fade) / fade
        val = int(32767 * 0.8 * env * math.sin(2 * math.pi * freq * i / rate))
        samples.extend([val] * channels)
    return struct.pack("<" + "h" * len(samples), *samples)


def extract_audio_onsets(mp4_path):
    tmp_wav = tempfile.mktemp(suffix=".wav")
    try:
        subprocess.run(
            [_ffmpeg_path(), "-y", "-i", mp4_path, "-ac", "1", "-ar", "48000", tmp_wav],
            capture_output=True, timeout=60, creationflags=0x08000000,
        )
        if not os.path.exists(tmp_wav):
            return []
        with wave.open(tmp_wav, "rb") as wf:
            rate = wf.getframerate()
            raw = wf.readframes(wf.getnframes())
        samples = struct.unpack("<" + "h" * (len(raw) // 2), raw)
        window = max(1, rate // 100)  # ~10ms windows
        energies = []
        for i in range(0, len(samples) - window, window):
            chunk = samples[i:i + window]
            energies.append(sum(s * s for s in chunk) / len(chunk))
        if not energies:
            return []
        threshold = max(energies) * 0.15
        min_gap = max(1, int(0.5 * rate / window))
        onsets = []
        last_i = -min_gap
        for i, e in enumerate(energies):
            if e >= threshold and (i - last_i) >= min_gap:
                onsets.append(i * window / rate)
                last_i = i
        return onsets
    finally:
        try:
            os.remove(tmp_wav)
        except Exception:
            pass


def extract_video_transitions(mp4_path, sample_fps=20):
    proc = subprocess.run(
        [_ffmpeg_path(), "-y", "-i", mp4_path, "-vf", f"fps={sample_fps},scale=2:2",
         "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
        capture_output=True, timeout=120, creationflags=0x08000000,
    )
    raw = proc.stdout
    frame_size = 2 * 2 * 3
    n_frames = len(raw) // frame_size
    colors = []
    for i in range(n_frames):
        chunk = raw[i * frame_size:(i + 1) * frame_size]
        r = sum(chunk[0::3])
        b = sum(chunk[2::3])
        colors.append("red" if r > b else "blue")
    transitions = []
    for i in range(1, len(colors)):
        if colors[i] != colors[i - 1]:
            transitions.append(i / sample_fps)
    return transitions


def run_analyze(mp4_path):
    print(f"Analyzing {mp4_path} ...")
    rc = 0

    # 1) Decode-corruption check — any decoder error means the muxed video is
    #    damaged (e.g. a still-being-written segment got included).
    errs = _decode_errors(mp4_path)
    if errs:
        print(f"  DECODE ERRORS (video corruption):\n    {errs[:400]}")
        rc = 1
    else:
        print("  decode: clean (no errors)")

    # 2) Audio-vs-video duration — a mismatch is exactly the overlap/desync
    #    symptom (audio timeline shorter/longer than the video it's muxed to).
    vdur = _stream_duration(mp4_path, "0:v:0")
    adur = _stream_duration(mp4_path, "0:a:0")
    if vdur is not None and adur is not None:
        diff = abs(vdur - adur)
        print(f"  video duration: {vdur:.3f}s  audio duration: {adur:.3f}s  diff: {diff * 1000:+.0f}ms")
        if diff > 0.25:
            print("  DURATION MISMATCH (audio vs video) — overlap/desync risk")
            rc = 1
    else:
        # A stream we can't measure (parse failed / 0-length) is itself a problem,
        # not something to pass silently.
        print(f"  DURATION UNMEASURABLE (video={vdur} audio={adur}) — treating as failure")
        rc = 1

    # 3) Event-based A/V offset (existing measurement).
    video_ts = extract_video_transitions(mp4_path)
    audio_ts = extract_audio_onsets(mp4_path)
    print(f"  video transitions detected: {len(video_ts)}")
    print(f"  audio onsets detected:      {len(audio_ts)}")
    if not video_ts or not audio_ts:
        print("ERROR: not enough detected events to measure sync.")
        return 1

    offsets_ms = []
    for vt in video_ts:
        closest = min(audio_ts, key=lambda at: abs(at - vt))
        if abs(closest - vt) < 0.5:
            offsets_ms.append((closest - vt) * 1000)

    if not offsets_ms:
        print("ERROR: no video/audio event pairs matched within tolerance.")
        return 1

    offsets_ms.sort()
    mean_off = sum(offsets_ms) / len(offsets_ms)
    median_off = offsets_ms[len(offsets_ms) // 2]
    print(f"  paired events: {len(offsets_ms)}")
    print(f"  mean offset:   {mean_off:+.1f} ms")
    print(f"  median offset: {median_off:+.1f} ms")
    print("  (positive = audio plays AFTER the video transition (audio behind/late);")
    print("   negative = audio plays BEFORE the video transition (audio ahead/early))")
    return rc


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
    out_stream = pa.open(
        format=cr.pyaudio.paInt16, channels=2, rate=TONE_RATE,
        output=True, output_device_index=out_dev["index"],
    )
    tone = make_tone_pcm()

    win = tk.Toplevel(root)
    win.overrideredirect(True)
    win.attributes("-topmost", True)
    win.geometry(f"{mon['w']}x{mon['h']}+{mon['x']}+{mon['y']}")
    win.configure(bg="red")
    win.deiconify()

    stop_flag = threading.Event()

    def flip_loop():
        start = time.monotonic()
        n = 0
        while not stop_flag.is_set():
            target = start + n * FLIP_INTERVAL
            now = time.monotonic()
            if target > now:
                time.sleep(target - now)
            color = "red" if n % 2 == 0 else "blue"
            try:
                win.after(0, lambda c=color: win.configure(bg=c))
            except Exception:
                pass
            try:
                out_stream.write(tone)
            except Exception:
                pass
            n += 1

    capture.start()
    print(f"Capturing at {args.fps} FPS, buffer_seconds={args.buffer}, monitor={mon_idx}, "
          f"repeats={args.repeats}...")
    threading.Thread(target=flip_loop, daemon=True).start()

    results = []
    changed = [False]

    def do_save():
        def on_success():
            folder = cr.get_output_folder(config)
            files = [os.path.join(folder, f) for f in os.listdir(folder) if f.startswith("Clip_")]
            path = max(files, key=os.path.getmtime) if files else None
            results.append(path)
            print(f"  save {len(results)}/{args.repeats} done: {path}")
            if len(results) >= args.repeats:
                stop_flag.set()
                root.after(500, root.quit)
            elif getattr(args, "change_buffer", None) and not changed[0]:
                # Exercise the actual failing path: change the clip duration
                # mid-session (as Settings Save does) and keep capturing/saving.
                changed[0] = True
                old = config["buffer_seconds"]
                config["buffer_seconds"] = args.change_buffer
                print(f"  >>> changing buffer_seconds {old} -> {args.change_buffer} "
                      f"via restart_video() (audio must stay untouched)")
                capture.restart_video()
                root.after(int((min(args.change_buffer, 20) + 8) * 1000), do_save)
            else:
                root.after(5000, do_save)

        print(f"Triggering save_replay() ({len(results) + 1}/{args.repeats})...")
        capture.save_replay(on_success=lambda: root.after(0, on_success))

    wait_ms = int((args.buffer + 10) * 1000)
    root.after(wait_ms, do_save)
    root.mainloop()

    stop_flag.set()
    try:
        out_stream.stop_stream()
        out_stream.close()
        pa.terminate()
    except Exception:
        pass
    try:
        win.destroy()
    except Exception:
        pass
    capture.cleanup()
    audio.cleanup()

    if not results or not all(results):
        print("ERROR: not all saves succeeded (save_replay never called on_success).")
        return 1

    exit_code = 0
    for i, path in enumerate(results, 1):
        print(f"\n=== Save {i}/{len(results)}: {path} ===")
        rc = run_analyze(path)
        exit_code = exit_code or rc
    return exit_code


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--generate", action="store_true", help="Run a live capture + save + analyze cycle")
    mode.add_argument("--analyze", metavar="MP4_PATH", help="Analyze an existing clip")
    parser.add_argument("--fps", type=int, default=60)
    parser.add_argument("--buffer", type=int, default=30, help="buffer_seconds to test")
    parser.add_argument("--monitor", type=int, default=0)
    parser.add_argument("--repeats", type=int, default=1,
                         help="Number of successive save_replay() calls within one session")
    parser.add_argument("--change-buffer", type=int, default=None, metavar="N",
                         help="After the first save, change buffer_seconds to N via "
                              "restart_video() and keep saving — tests the duration-change "
                              "path (audio must stay in sync). Implies --repeats>=2.")
    args = parser.parse_args()

    if args.change_buffer and args.repeats < 2:
        args.repeats = 2

    if args.analyze:
        sys.exit(run_analyze(args.analyze))
    else:
        sys.exit(run_generate(args))


if __name__ == "__main__":
    main()
