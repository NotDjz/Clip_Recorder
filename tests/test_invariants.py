"""
Guard-rail tests for Clip Recorder.

Every assertion here encodes an invariant this project already paid for with a
real bug. They exist so a future change cannot quietly break one of them again.

No capture, no audio device, no real ffmpeg: the ffmpeg command lines and the
segment-selection logic are exercised by faking subprocess and the tk root, so
the whole file runs in well under a second.

    py tests\\test_invariants.py
"""
import importlib.util
import inspect
import os
import shutil
import tempfile
import time

REPO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP_PATH = os.path.join(REPO_DIR, "src", "clip_recorder.pyw")

spec = importlib.util.spec_from_file_location("clip_recorder", APP_PATH)
cr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cr)


# ─── Fakes ───────────────────────────────────────────────────────────────────

class FakeRoot:
    def after(self, ms, fn=None, *a):
        return "after-id"

    def after_cancel(self, ident):
        pass


class FakeProc:
    def __init__(self):
        self.stdin = self
        self.returncode = 0

    def poll(self):
        return None            # "still running"

    def write(self, b):
        pass

    def flush(self):
        pass

    def wait(self, timeout=None):
        return 0

    def terminate(self):
        pass

    def kill(self):
        pass


class FakeCompleted:
    returncode = 0
    stdout = b""
    stderr = b""


class FakeAudio:
    """Mimics just enough of AudioCapture for save_replay's logging + save_wav."""
    available = True
    mic_available = False

    def __init__(self):
        import threading
        self._loopback_lock = threading.Lock()
        self._mic_lock = threading.Lock()
        self._loopback_chunks = []
        self._mic_chunks = []
        self._loopback_bytes = 0
        self._mic_bytes = 0
        self._loopback_frames = 0
        self._mic_frames = 0
        self._loopback_status_flags = 0
        self._mic_status_flags = 0
        self._loopback_stream = None
        self._mic_stream = None
        self._rate = 48000
        self._mic_rate = 48000
        self._channels = 2
        self._mic_channels = 2
        self._sample_width = 2
        self.started = 0
        self.stopped = 0
        self.save_calls = []

    @staticmethod
    def _stream_alive(stream):
        return False

    def start(self):
        self.started += 1

    def stop(self):
        self.stopped += 1

    def save_wav(self, path, t_end, duration):
        self.save_calls.append(("loopback", t_end, duration))
        # Must really land on disk: save_replay picks the audio branch via
        # os.path.exists(), so a fake that skips this silently exercises only
        # the no-audio path and leaves the mux branch untested.
        with open(path, "wb") as f:
            f.write(b"RIFF")
        return True

    def save_mic_wav(self, path, t_end, duration):
        self.save_calls.append(("mic", t_end, duration))
        with open(path, "wb") as f:
            f.write(b"RIFF")
        return True


class Recorder:
    """Captures the ffmpeg command lines instead of running them."""

    def __init__(self):
        self.popen_cmds = []
        self.run_cmds = []
        self.concat_lists = []
        self.concat_marks = []

    def install(self):
        self._popen, self._run, self._thread = cr.subprocess.Popen, cr.subprocess.run, cr.threading.Thread
        rec = self

        def fake_popen(cmd, *a, **k):
            rec.popen_cmds.append(cmd)
            return FakeProc()

        def fake_run(cmd, *a, **k):
            rec.run_cmds.append(cmd)
            # Read the concat list NOW: _run()'s finally block deletes it.
            # Snapshots are renamed copies, so identify what actually went into
            # the clip by CONTENT (seed_segments writes a per-segment marker),
            # not by filename.
            if "concat" in cmd and "-i" in cmd:
                try:
                    with open(cmd[cmd.index("-i") + 1], encoding="utf-8") as f:
                        entries = [l.split("'")[1] for l in f if l.startswith("file ")]
                    rec.concat_lists.append(entries)
                    marks = []
                    for e in entries:
                        with open(e, "rb") as sf:
                            marks.append(sf.read().split(b"|")[0].decode("ascii", "replace"))
                    rec.concat_marks.append(marks)
                except Exception:
                    pass
            return FakeCompleted()

        class SyncThread:                      # run save_replay's worker inline
            def __init__(self, target=None, args=(), kwargs=None, daemon=None):
                self._t, self._a, self._k = target, args, kwargs or {}

            def start(self):
                if self._t:
                    self._t(*self._a, **self._k)

        cr.subprocess.Popen, cr.subprocess.run, cr.threading.Thread = fake_popen, fake_run, SyncThread

    def restore(self):
        cr.subprocess.Popen, cr.subprocess.run, cr.threading.Thread = self._popen, self._run, self._thread


def make_capture(fps=60, buffer_seconds=15, ddagrab=True, nvenc=True, audio=None, monitor=0):
    cap = cr.FFmpegCapture.__new__(cr.FFmpegCapture)   # skip __init__: no device probing
    cap.root = FakeRoot()
    cap.config = dict(cr.DEFAULTS)
    cap.config.update(monitor=monitor, fps=fps, buffer_seconds=buffer_seconds)
    cap.monitors = [{"name": "M1", "x": 0, "y": 0, "w": 1920, "h": 1080, "primary": True},
                    {"name": "M2", "x": 1920, "y": 0, "w": 2560, "h": 1440, "primary": False}]
    cap.audio = audio
    cap.proc = None
    cap.segment_dir = tempfile.mkdtemp(prefix="inv_test_")   # NOT cliprec_*: see CLAUDE.md
    cap.has_nvenc = nvenc
    cap.has_ddagrab = ddagrab
    cap._poll_id = None
    return cap


def seed_segments(cap, count, spacing=None, newest_age=0.3):
    """Create `count` seg_*.ts files whose mtimes are `spacing` apart, the newest
    being `newest_age` seconds old. Returns the list of names, oldest first."""
    spacing = spacing if spacing is not None else cr.SEGMENT_DURATION
    now = time.time()
    names = []
    for i in range(count):
        name = f"seg_{i:03d}.ts"
        path = os.path.join(cap.segment_dir, name)
        with open(path, "wb") as f:
            # Marker so a snapshot can be traced back to its source segment.
            f.write(f"SEG{i:03d}".encode() + b"|" + b"\0" * 128)
        age = newest_age + (count - 1 - i) * spacing
        os.utime(path, (now - age, now - age))
        names.append(name)
    return names


def concat_entries(cap, rec):
    """The files the concat list actually pointed at, captured at call time."""
    return rec.concat_lists[0] if rec.concat_lists else []


def arg_after(cmd, flag):
    return cmd[cmd.index(flag) + 1] if flag in cmd else None


def video_cmd(rec):
    for cmd in rec.run_cmds:
        if "concat" in cmd:
            return cmd
    return None


# ─── Video pipeline ──────────────────────────────────────────────────────────

def test_keyframe_interval_equals_fps_times_segment_duration():
    """Concat produces visual artifacts unless every segment starts on a keyframe."""
    for fps in cr.FPS_OPTIONS:
        cap = make_capture(fps=fps)
        rec = Recorder(); rec.install()
        try:
            cap._start_ffmpeg()
        finally:
            rec.restore(); shutil.rmtree(cap.segment_dir, ignore_errors=True)
        cmd = rec.popen_cmds[0]
        assert arg_after(cmd, "-g") == str(fps * cr.SEGMENT_DURATION), \
            f"fps={fps}: -g is {arg_after(cmd, '-g')}, expected {fps * cr.SEGMENT_DURATION}"
        assert arg_after(cmd, "-segment_time") == str(cr.SEGMENT_DURATION)


def test_capture_flushes_packets_so_snapshots_are_current():
    cap = make_capture()
    rec = Recorder(); rec.install()
    try:
        cap._start_ffmpeg()
    finally:
        rec.restore(); shutil.rmtree(cap.segment_dir, ignore_errors=True)
    assert arg_after(rec.popen_cmds[0], "-flush_packets") == "1"


def test_segment_wrap_covers_the_requested_buffer():
    for buf in cr.BUFFER_OPTIONS:
        cap = make_capture(buffer_seconds=buf)
        rec = Recorder(); rec.install()
        try:
            cap._start_ffmpeg()
        finally:
            rec.restore(); shutil.rmtree(cap.segment_dir, ignore_errors=True)
        wrap = int(arg_after(rec.popen_cmds[0], "-segment_wrap"))
        assert wrap * cr.SEGMENT_DURATION >= buf, f"buffer {buf}s not covered by {wrap} segments"


def test_ddagrab_nvenc_has_no_pixel_format_filter():
    """-vf format=yuv420p with ddagrab+NVENC fails with 'Invalid argument'."""
    cap = make_capture(ddagrab=True, nvenc=True)
    rec = Recorder(); rec.install()
    try:
        cap._start_ffmpeg()
    finally:
        rec.restore(); shutil.rmtree(cap.segment_dir, ignore_errors=True)
    cmd = " ".join(rec.popen_cmds[0])
    assert "format=yuv420p" not in cmd, "ddagrab+NVENC must not carry a pixel-format filter"
    assert "ddagrab" in cmd and "output_idx" in cmd


def test_gdigrab_selects_the_monitor_by_offset():
    cap = make_capture(ddagrab=False, monitor=1)
    rec = Recorder(); rec.install()
    try:
        cap._start_ffmpeg()
    finally:
        rec.restore(); shutil.rmtree(cap.segment_dir, ignore_errors=True)
    cmd = rec.popen_cmds[0]
    assert arg_after(cmd, "-offset_x") == "1920" and arg_after(cmd, "-video_size") == "2560x1440"


# ─── Segment selection ───────────────────────────────────────────────────────

def test_in_progress_segment_is_never_selected():
    """Snapshotting the file ffmpeg is still writing yields a torn frame.
    Traced by content marker: the snapshot copies are renamed, so a filename
    check here would silently never detect the regression."""
    count = 8
    cap = make_capture(buffer_seconds=5)
    seed_segments(cap, count)
    rec = Recorder(); rec.install()
    try:
        cap.save_replay()
    finally:
        rec.restore()
    marks = rec.concat_marks[0] if rec.concat_marks else []
    shutil.rmtree(cap.segment_dir, ignore_errors=True)
    assert marks, "no concat list was produced"
    newest = f"SEG{count - 1:03d}"
    assert newest not in marks, f"the in-progress segment ({newest}) was included: {marks}"


def test_concat_references_only_snapshot_copies():
    """The live process cyclically overwrites seg_*.ts; only copies are safe."""
    cap = make_capture(buffer_seconds=5)
    seed_segments(cap, 8)
    rec = Recorder(); rec.install()
    try:
        cap.save_replay()
    finally:
        rec.restore()
    entries = concat_entries(cap, rec)
    shutil.rmtree(cap.segment_dir, ignore_errors=True)
    assert entries
    for e in entries:
        base = os.path.basename(e)
        assert base.startswith("snap_"), f"concat points at a live file: {base}"


def test_never_seeks_into_a_segment():
    """Whole segments only: a fractional -ss lands on the wrong real keyframe.
    Checked on BOTH branches — save_replay builds a different command when there
    is audio to mux than when there is none."""
    for audio in (None, FakeAudio()):
        cap = make_capture(buffer_seconds=5, audio=audio)
        seed_segments(cap, 8)
        rec = Recorder(); rec.install()
        try:
            cap.save_replay()
        finally:
            rec.restore()
        cmds = [c for c in rec.run_cmds if "concat" in c]
        shutil.rmtree(cap.segment_dir, ignore_errors=True)
        assert cmds, "no concat command was issued"
        for cmd in cmds:
            assert arg_after(cmd, "-ss") == "0", \
                f"save_replay seeks into a segment (audio={audio is not None})"


def test_audio_window_matches_the_video_duration():
    """Both sides must agree on the same real window, or they drift apart."""
    audio = FakeAudio()
    cap = make_capture(buffer_seconds=5, audio=audio)
    seed_segments(cap, 8)
    rec = Recorder(); rec.install()
    try:
        cap.save_replay()
    finally:
        rec.restore()
    cmd = video_cmd(rec)
    shutil.rmtree(cap.segment_dir, ignore_errors=True)
    assert audio.save_calls, "audio was never rendered"
    _, _, duration = audio.save_calls[0]
    assert abs(duration - float(arg_after(cmd, "-t"))) < 1e-6, \
        "audio duration differs from the video -t"


def test_fallback_anchors_at_the_oldest_segment_open_time():
    """Not enough history: the window starts where the oldest segment OPENS,
    not where it closes — anchoring on its mtime cost one SEGMENT_DURATION of
    A/V offset on every save made before the buffer refilled."""
    audio = FakeAudio()
    cap = make_capture(buffer_seconds=120, audio=audio)   # far more than we seed
    seed_segments(cap, 4, newest_age=0.3)
    oldest_mtime = min(os.path.getmtime(os.path.join(cap.segment_dir, f))
                       for f in os.listdir(cap.segment_dir))
    rec = Recorder(); rec.install()
    try:
        cap.save_replay()
    finally:
        rec.restore()
    shutil.rmtree(cap.segment_dir, ignore_errors=True)
    _, t_end, duration = audio.save_calls[0]
    window_start = t_end - duration
    assert abs(window_start - (oldest_mtime - cr.SEGMENT_DURATION)) < 0.05, \
        "fallback window does not start at the oldest segment's open time"


def test_aborts_when_only_the_in_progress_segment_exists():
    cap = make_capture()
    seed_segments(cap, 1)
    rec = Recorder(); rec.install()
    try:
        cap.save_replay()
    finally:
        rec.restore()
    ran = list(rec.run_cmds)
    shutil.rmtree(cap.segment_dir, ignore_errors=True)
    assert not ran, "save_replay must abort instead of muxing a torn segment"


# ─── Restart / cleanup behaviour ─────────────────────────────────────────────

def test_restart_video_does_not_touch_audio():
    """Restarting audio re-exposes the flaky WASAPI loopback (re)open — the
    direct trigger of the duration-change desync."""
    audio = FakeAudio()
    cap = make_capture(audio=audio)
    rec = Recorder(); rec.install()
    try:
        cap.restart_video()
    finally:
        rec.restore(); shutil.rmtree(cap.segment_dir, ignore_errors=True)
    assert audio.started == 0 and audio.stopped == 0, \
        "restart_video touched the audio streams"


def test_wipe_segments_spares_an_in_flight_save():
    """A settings change must not delete the temp files a running save owns."""
    cap = make_capture()
    for name in ("seg_000.ts", "snap_abc_000.ts", "concat_abc.txt",
                 "loopback_abc.wav", "video_abc.mp4"):
        open(os.path.join(cap.segment_dir, name), "wb").close()
    cap._wipe_segments()
    left = set(os.listdir(cap.segment_dir))
    shutil.rmtree(cap.segment_dir, ignore_errors=True)
    assert "seg_000.ts" not in left, "capture segments should be wiped"
    assert {"snap_abc_000.ts", "concat_abc.txt", "loopback_abc.wav",
            "video_abc.mp4"} <= left, "wipe destroyed files an in-flight save needs"


# ─── Source-level guarantees ─────────────────────────────────────────────────

def test_audio_callbacks_do_no_blocking_io():
    """PortAudio callbacks are realtime: file I/O there causes dropouts."""
    for fn in (cr.AudioCapture._loopback_callback, cr.AudioCapture._mic_callback):
        src = inspect.getsource(fn)
        assert "log(" not in src, f"{fn.__name__} performs logging (file I/O)"
        assert "open(" not in src, f"{fn.__name__} opens a file"


def test_uninstall_removes_known_files_and_never_recurses():
    src = inspect.getsource(cr)
    assert "-Recurse" not in src, "uninstall must never recurse over a folder"
    ps = src[src.index("Wait-Process"):src.index("Wait-Process") + 1200]
    for target in ("exe_path", "CONFIG_FILE", "LOG_FILE", "desktop_lnk"):
        assert target in ps, f"uninstall no longer removes {target}"


def test_settings_has_no_apply_button():
    """A separate Apply was removed as a confusing strict subset of Save.
    (Reads the module source: inspect.getsource on a class defined in a .pyw
    raises 'is a built-in class', though methods and the module itself work.)"""
    assert 'text="Apply"' not in inspect.getsource(cr)


def test_mix_resamples_before_amix_with_normalize_off():
    """Without this the mix pumps in volume or buzzes."""
    src = inspect.getsource(cr.FFmpegCapture.save_replay)
    assert "aresample=48000" in src and "normalize=0" in src


def test_log_is_capped():
    original, cap_bytes = cr.LOG_FILE, cr.LOG_MAX_BYTES
    cr.LOG_FILE = os.path.join(tempfile.gettempdir(), "inv_logcap.log")
    cr.LOG_MAX_BYTES = 64 * 1024
    try:
        if os.path.exists(cr.LOG_FILE):
            os.remove(cr.LOG_FILE)
        for i in range(4000):
            cr.log(f"entry {i:05d} " + "x" * 60)
        size = os.path.getsize(cr.LOG_FILE)
        with open(cr.LOG_FILE, encoding="utf-8") as f:
            lines = f.read().splitlines()
        os.remove(cr.LOG_FILE)
    finally:
        cr.LOG_FILE, cr.LOG_MAX_BYTES = original, cap_bytes
    assert size <= 64 * 1024, "log exceeded its cap"
    assert lines[0] == "[log truncated]"
    assert "entry 03999" in lines[-1], "the newest entries were not the ones kept"
    assert lines[1].startswith("[20"), "a partial line was left at the cut point"


def main():
    tests = [v for k, v in sorted(globals().items())
             if k.startswith("test_") and callable(v)]
    failed = []
    for t in tests:
        try:
            t()
            print(f"  PASS  {t.__name__}")
        except AssertionError as e:
            failed.append(t.__name__)
            print(f"  FAIL  {t.__name__}: {e}")
        except Exception as e:                 # a broken test must not abort the suite
            failed.append(t.__name__)
            print(f"  ERROR {t.__name__}: {type(e).__name__}: {e}")
    print(f"\n{len(tests) - len(failed)}/{len(tests)} invariants hold")
    if failed:
        print("broken:", ", ".join(failed))
    raise SystemExit(1 if failed else 0)


if __name__ == "__main__":
    main()
