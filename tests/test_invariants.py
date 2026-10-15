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
import sys
import tempfile
import threading
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
        self.pid = 424242

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

    def __init__(self, fail_on=None, timeout_on=None):
        self.popen_cmds = []
        self.popen_kwargs = []
        self.run_cmds = []
        self.concat_lists = []
        self.concat_marks = []
        self.fail_on = fail_on       # a command containing this returns code 1
        self.timeout_on = timeout_on # ... or raises TimeoutExpired

    def install(self):
        self._popen, self._run, self._thread = cr.subprocess.Popen, cr.subprocess.run, cr.threading.Thread
        rec = self

        def fake_popen(cmd, *a, **k):
            rec.popen_cmds.append(cmd)
            rec.popen_kwargs.append(k)
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
            done = FakeCompleted()
            joined = " ".join(map(str, cmd))
            if "concat" in cmd and os.path.basename(str(cmd[-1])).startswith("video_"):
                # The intermediate video-only file, or the mux is never reached.
                # Never the clip itself: tests rely on output_path not appearing.
                with open(cmd[-1], "wb") as f:
                    f.write(b"\0")
            if "amix" in joined:
                # A real amix writes its WAV even when it then fails part-way:
                # the partial-file case is exactly what must not be trusted.
                with open(cmd[-1], "wb") as f:
                    f.write(b"RIFF")
            if rec.fail_on and rec.fail_on in joined:
                done.returncode = 1
            if rec.timeout_on and rec.timeout_on in joined:
                raise cr.subprocess.TimeoutExpired(cmd, 30)
            return done

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
    cap._capture_started = 0.0   # __init__ is skipped above; mirror it
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


def test_nvenc_b_frames_never_hold_output_back():
    """B-frames only where measured, never without -delay 0 (see _start_ffmpeg)."""
    for fps in cr.FPS_OPTIONS:
        cap = make_capture(fps=fps, ddagrab=True, nvenc=True)
        rec = Recorder(); rec.install()
        try:
            cap._start_ffmpeg()
        finally:
            rec.restore(); shutil.rmtree(cap.segment_dir, ignore_errors=True)
        cmd = rec.popen_cmds[0]
        if fps >= cr.B_FRAMES_MIN_FPS:
            assert "-bf" in cmd and arg_after(cmd, "-delay") == "0", \
                f"fps={fps}: B-frames without -delay 0 make the video lag the audio"
        else:
            assert "-bf" not in cmd, \
                f"fps={fps}: B-frames lag 3 frames, {3000 // fps} ms here, never measured"


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
    """Whole segments only, and no -ss at all: a fractional -ss lands on the
    wrong real keyframe, and even -ss 0 drops the first B-frame segment (see
    save_replay). Checked with and without audio to mux, which change the
    command's output."""
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
            assert "-ss" not in cmd, \
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

def test_capture_records_its_ffmpeg_pid():
    """Without the PID on disk, a crashed instance's ffmpeg can never be found
    again — it keeps an NVENC session and every later launch records nothing."""
    cap = make_capture()
    rec = Recorder(); rec.install()
    try:
        cap._start_ffmpeg()
    finally:
        rec.restore()
    pid_path = os.path.join(cap.segment_dir, cr.FFMPEG_PID_FILE)
    exists = os.path.exists(pid_path)
    content = open(pid_path).read().strip() if exists else ""
    shutil.rmtree(cap.segment_dir, ignore_errors=True)
    assert exists, "the capture did not record its ffmpeg PID"
    assert content == "424242", f"wrong PID recorded: {content!r}"


def test_orphan_cleanup_never_kills_a_foreign_process():
    """PIDs are reused. Killing whatever now holds a recorded PID would be far
    worse than leaving an orphan, so the image name is checked first."""
    import subprocess as sp
    victim = sp.Popen([sys.executable, "-c", "import time; time.sleep(30)"],
                      creationflags=0x08000000)
    d = tempfile.mkdtemp(prefix="inv_test_")
    try:
        with open(os.path.join(d, cr.FFMPEG_PID_FILE), "w") as f:
            f.write(str(victim.pid))          # a python.exe, not our ffmpeg
        cr._kill_orphan_ffmpeg(d)
        time.sleep(0.4)
        survived = victim.poll() is None
    finally:
        victim.kill()
        shutil.rmtree(d, ignore_errors=True)
    assert survived, "orphan cleanup killed a process that was not our ffmpeg"


def test_orphan_cleanup_tolerates_a_missing_or_bogus_pid_file():
    d = tempfile.mkdtemp(prefix="inv_test_")
    try:
        cr._kill_orphan_ffmpeg(d)                       # no pid file at all
        with open(os.path.join(d, cr.FFMPEG_PID_FILE), "w") as f:
            f.write("not-a-number")
        cr._kill_orphan_ffmpeg(d)                       # garbage
        with open(os.path.join(d, cr.FFMPEG_PID_FILE), "w") as f:
            f.write("999999999")
        cr._kill_orphan_ffmpeg(d)                       # long-dead PID
    finally:
        shutil.rmtree(d, ignore_errors=True)


def test_wipe_segments_keeps_the_pid_file():
    """restart_video() wipes segments; losing the PID there would strand the
    ffmpeg it just replaced."""
    cap = make_capture()
    open(os.path.join(cap.segment_dir, "seg_000.ts"), "wb").close()
    with open(os.path.join(cap.segment_dir, cr.FFMPEG_PID_FILE), "w") as f:
        f.write("1234")
    cap._wipe_segments()
    left = set(os.listdir(cap.segment_dir))
    shutil.rmtree(cap.segment_dir, ignore_errors=True)
    assert cr.FFMPEG_PID_FILE in left, "the ffmpeg PID file was wiped"


def test_audio_callbacks_do_no_blocking_io():
    """PortAudio callbacks are realtime: file I/O there causes dropouts."""
    for fn in (cr.AudioCapture._loopback_callback, cr.AudioCapture._mic_callback):
        src = inspect.getsource(fn)
        assert "log(" not in src, f"{fn.__name__} performs logging (file I/O)"
        assert "open(" not in src, f"{fn.__name__} opens a file"


def test_job_object_is_kill_on_close_with_explicit_breakaway():
    """These are the documented Win32 values. A typo in any of them is a silent
    no-op — the job is created, reports success, and simply does not do what its
    name says, which is the exact failure mode the job exists to prevent."""
    assert cr.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE == 0x00002000
    assert cr.JOB_OBJECT_LIMIT_BREAKAWAY_OK == 0x00000800
    assert cr.CREATE_BREAKAWAY_FROM_JOB == 0x01000000
    assert cr.JOBOBJECT_EXTENDED_LIMIT_INFORMATION_CLASS == 9
    src = inspect.getsource(cr._ensure_kill_on_close_job)
    assert "JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE" in src, "the job must kill on close"
    assert "JOB_OBJECT_LIMIT_BREAKAWAY_OK" in src, "uninstall needs an escape hatch"
    assert "SILENT_BREAKAWAY" not in src, \
        "silent breakaway would let the ffmpegs escape too, defeating the job"


def test_capture_joins_the_job_before_spawning_ffmpeg():
    """Establishing the job after a child is spawned would leave that child
    outside it — including the two probes in __init__."""
    src = inspect.getsource(cr.FFmpegCapture.__init__)
    assert "_ensure_kill_on_close_job()" in src, \
        "capture must join the kill-on-close job"
    job_at = src.index("_ensure_kill_on_close_job()")
    # Both probes, not just the first: asserting only on detect_nvenc() passed
    # while detect_ddagrab() could be reordered above the job and spawn its
    # ffmpeg outside it.
    for probe in ("detect_nvenc()", "detect_ddagrab()"):
        assert job_at < src.index(probe), \
            f"the job must be established before {probe} spawns ffmpeg"


def test_orphan_sweep_isolates_each_directory():
    """One failing orphan must not abort the sweep for the rest."""
    src = inspect.getsource(cr.main)
    start = src.index("for d in os.listdir(tmp)")
    # Bound the slice at the sweep's OWN handler, not a byte count: a fixed
    # window overran into the ffmpeg -version check below, which has a `try:`
    # of its own, so the assertion held only by 21 characters of luck.
    loop = src[start:src.index("\n    except Exception:", start)]
    assert "_kill_orphan_ffmpeg" in loop, "the sweep no longer kills orphans"
    assert "try:" in loop, "the orphan kill is not guarded inside the loop"
    assert loop.index("try:") < loop.index("_kill_orphan_ffmpeg"), \
        "the guard must sit inside the loop, not around it"


def test_uninstall_helper_breaks_away_from_the_job():
    """The helper waits for THIS process to exit before deleting the exe, so in
    a KILL_ON_JOB_CLOSE job it dies with us and uninstall silently does nothing
    at all — no error, no deletion."""
    src = inspect.getsource(cr)
    spawn = src[src.index("Wait-Process"):src.index("Wait-Process") + 1500]
    # Assert on the creationflags expression itself, not on the name appearing
    # somewhere nearby: the comment above the call names the flag too, and
    # matching that would make this test unable to fail (mutation_check caught
    # exactly that when this test was first written).
    flags = next(l for l in spawn.splitlines() if "creationflags=" in l)
    assert "CREATE_BREAKAWAY_FROM_JOB" in flags, \
        "the uninstall helper must break away or the job kills it with us"


def test_uninstall_removes_known_files_and_never_recurses():
    src = inspect.getsource(cr)
    assert "-Recurse" not in src, "uninstall must never recurse over a folder"
    ps = src[src.index("Wait-Process"):src.index("Wait-Process") + 1200]
    # startup_lnk is the one that hurts most if forgotten: Windows would try to
    # launch a deleted exe at every sign-in, with the app gone to fix it.
    for target in ("exe_path", "CONFIG_FILE", "LOG_FILE", "desktop_lnk",
                   "startup_lnk"):
        assert target in ps, f"uninstall no longer removes {target}"


def test_banner_keeps_its_window_styles():
    """The banner is an overlay drawn over games and excluded from the clip.

    WS_EX_NOACTIVATE is what stops it stealing foreground from a windowed or
    borderless game when it appears. WDA_EXCLUDEFROMCAPTURE (0x11) is what keeps
    it out of the recorded MP4 — lose it and every clip carries the banner burnt
    into the picture. Both are set while the window is still withdrawn, because
    a style applied after the first deiconify() arrives a frame too late."""
    styles = inspect.getsource(cr.NotificationBanner._apply_window_styles)
    for flag in ("WS_EX_TRANSPARENT", "WS_EX_TOOLWINDOW", "WS_EX_NOACTIVATE"):
        assert flag in styles, f"the banner no longer sets {flag}"
    assert "0x00000011" in styles, (
        "WDA_EXCLUDEFROMCAPTURE is gone; the banner would be recorded")
    show = inspect.getsource(cr.NotificationBanner.show)
    order = [show.index(s) for s in ("withdraw()", "_apply_window_styles()",
                                     "deiconify()")]
    assert order == sorted(order), (
        "styles must be applied while withdrawn, before the first deiconify()")


def test_settings_vars_match_what_apply_parses():
    """_apply() PARSES what _build() puts in these StringVars.

    A redesign that changes a display format breaks device selection with no
    exception at all — just the wrong monitor, or the microphone silently pinned
    to a device literally named "(Auto — system default)"."""
    build = inspect.getsource(cr.SettingsWindow._build)
    apply_ = inspect.getsource(cr.SettingsWindow._apply)

    # monitor: "N: name (WxH)" read back as int(value.split(":")[0]) - 1
    assert 'split(":")' in apply_, "the monitor parse changed"
    assert 'f"{i+1}: ' in build, "the monitor label no longer starts with 'N: '"

    # the auto sentinel, produced by both device pickers and matched by prefix
    assert 'startswith("(Auto")' in apply_, "the auto sentinel check changed"
    # On the two real expressions, not a count: a comment mentioning the
    # sentinel inflated the count and made this fail for the wrong reason -
    # the same 'matched a name sitting nearby' trap CLAUDE.md warns about.
    for devices in ("loopback_devices", "mic_devices"):
        expected = '["(Auto — system default)"] + ' + devices
        assert expected in build, (
            f"the {devices} picker no longer offers the sentinel _apply() parses")

    # recoloured via .config(bg=, fg=) during capture, which ttk widgets refuse
    assert "self.hotkey_btn = tk.Button(" in build, \
        "hotkey_btn must stay a classic tk.Button"


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



def test_giving_up_reports_a_reason():
    """A save that gives up used to return having told nobody. The hotkey now
    puts a "Saving clip" banner on screen before the work starts, so a silent
    abort leaves it sitting over the game. Three of the give-up paths are
    driven here, and their reasons must stay distinguishable: one tells the
    user to wait, the others say the clip was lost."""
    def reasons_for(segment_count, drop_dir=False, just_started=False):
        cap = make_capture(buffer_seconds=5)
        if just_started:
            cap._capture_started = time.time()
        seed_segments(cap, segment_count)
        if drop_dir:
            shutil.rmtree(cap.segment_dir, ignore_errors=True)
        seen = []
        try:
            cap.save_replay(on_failure=seen.append)
        finally:
            shutil.rmtree(cap.segment_dir, ignore_errors=True)
        return seen

    # listdir itself raises: the segment directory went away under us.
    assert reasons_for(0, drop_dir=True) == ["error"], "the listdir abort said nothing"
    # Nothing captured at all. Not "wait a moment" - on a stale NVENC session
    # this is permanent, and telling the user to wait would be a lie.
    assert reasons_for(0) == ["error"], "the no-segment abort said nothing"
    # The same empty directory moments after a restart: ffmpeg has simply not
    # written seg_000 yet, so here waiting really is the advice. Telling these
    # two apart is the whole reason a reason exists.
    assert reasons_for(0, just_started=True) == ["too_soon"], \
        "an empty directory just after a restart was called a lost clip"
    # Only the in-progress segment exists: pressed too soon after a restart.
    assert reasons_for(1) == ["too_soon"], "the too-soon abort said nothing"


def test_a_superseded_banner_stops_pulsing():
    """The saving banner breathes its record dot on an after() chain. When a
    second banner replaces it, the old chain must retire - otherwise it keeps
    recolouring the dot that now belongs to its replacement, and "Clip saved"
    sits there breathing as though it were still working.

    The window below is deliberately alive, so dropping the generation check
    reaches self._canvas (None) and raises, instead of being masked by the
    window test that follows it."""
    class LiveWindow:
        def winfo_exists(self):
            return True

    class FakeRootWithAfter:
        def __init__(self):
            self.cancelled = []

        def after(self, ms, fn, *a):
            return "tick"

        def after_cancel(self, ident):
            self.cancelled.append(ident)

    class FakeCanvas:
        def __init__(self):
            self.fills = []

        def itemconfig(self, item, fill):
            self.fills.append(fill)

    b = cr.NotificationBanner.__new__(cr.NotificationBanner)
    b.root = FakeRootWithAfter()
    b._win, b._canvas, b._dot = LiveWindow(), FakeCanvas(), "dot"
    b._pulse_gen, b._pulse_id = 0, None

    # A live chain recolours the dot and books its next tick.
    b._pulse(0, 0)
    assert b._canvas.fills, "a live pulse did not recolour the dot"
    assert b._pulse_id == "tick", "a live pulse did not schedule its next tick"

    # Replacing the banner must retire that generation. Without this half, the
    # comparison in _pulse is permanently true and guards nothing.
    retired = b._pulse_gen
    b._stop_pulse()
    assert b._pulse_gen != retired, "_stop_pulse left the generation untouched"
    assert b._pulse_id is None and b.root.cancelled == ["tick"]

    # The retired chain must now be inert even though the window is still alive.
    b._canvas.fills.clear()
    b._pulse(retired, 1)
    assert not b._canvas.fills, "a retired chain recoloured its successor's dot"
    assert b._pulse_id is None, "a retired chain rescheduled itself"

def test_chimes_never_play_on_the_calling_thread():
    """winsound refuses SND_ASYNC together with SND_MEMORY, so every play is
    synchronous and has to be handed to its own thread. The press chime fires
    on the tk thread from do_save, so playing it inline would stall the whole
    interface for the length of the sound - and the save chime would sit in
    front of the banner it is supposed to accompany."""
    played = []
    real = cr.winsound.PlaySound
    cr.winsound.PlaySound = lambda data, flags: played.append(
        threading.current_thread())
    try:
        cr.play_start_sound()
        cr.play_save_sound()
        cr.play_fail_sound()
        deadline = time.time() + 3
        while len(played) < 3 and time.time() < deadline:
            time.sleep(0.01)
    finally:
        cr.winsound.PlaySound = real

    assert len(played) == 3, f"a chime never reached winsound: {played}"
    for t in played:
        assert t is not threading.main_thread(), \
            "a chime played on the calling thread"


def test_the_hotkey_acknowledges_the_press():
    """The chime and the saving banner ARE the feature, and nothing pinned that
    do_save fires them: the chime test calls play_start_sound() itself, so
    deleting the call from do_save left every harness green."""
    src = inspect.getsource(cr.main)
    body = src[src.index("def do_save("):]
    # Relies on do_save being followed by another 4-space def inside main()
    # (shutdown). If that stops being true .index raises, so this fails loudly
    # rather than quietly passing on an empty slice.
    body = body[:body.index("\n    def ")]
    assert "play_start_sound()" in body, "do_save no longer plays the press chime"
    assert 'state="saving"' in body, "do_save no longer shows the saving banner"


def test_a_lost_clip_is_heard_as_well_as_seen():
    """The press chime ascends and, without a matching descent, never resolves
    when the clip is lost: the only report is a pill in the corner, which is
    exactly where the user is not looking. Nothing else pins that show_failure
    plays it - the chime test calls play_fail_sound() itself."""
    src = inspect.getsource(cr.main)
    body = src[src.index("def show_failure("):]
    # Relies on show_failure being followed by another 4-space def inside
    # main() (start_save); if that stops being true .index raises, so this
    # fails loudly rather than passing on an empty slice.
    body = body[:body.index("\n    def ")]
    assert "play_fail_sound()" in body, "show_failure no longer plays the failure chime"


def test_a_failed_encode_reports_instead_of_going_quiet():
    """The give-up path most likely to be taken for real is the worker's own:
    concat, mix or mux fails and no clip appears. The three synchronous aborts
    were covered; this one was not, and it is the case that would otherwise
    leave a "Saving clip" banner expiring with no clip and no explanation."""
    cap = make_capture(buffer_seconds=5)
    seed_segments(cap, 8)
    seen = []
    rec = Recorder(); rec.install()
    try:
        # The fake ffmpeg writes nothing, so output_path never appears.
        cap.save_replay(on_failure=seen.append)
        deadline = time.time() + 5
        while not seen and time.time() < deadline:
            time.sleep(0.02)
    finally:
        rec.restore()
        shutil.rmtree(cap.segment_dir, ignore_errors=True)
    assert seen == ["error"], f"a failed encode reported {seen!r}"


def test_a_restart_never_mixes_two_captures():
    """_check_health() restarts a dead ffmpeg through start(). Its segments,
    the last one cut mid-write, must not survive into the new capture: a save
    would select both, and the mtime span would include the restart gap. Nor
    its segment list, whose times belong to the old capture clock."""
    cap = make_capture()
    old = seed_segments(cap, 5) + [cr.SEGMENT_LIST_FILE]
    with open(os.path.join(cap.segment_dir, cr.SEGMENT_LIST_FILE), "w") as f:
        f.write("seg_000.ts,0.000000,1.000000\n")
    rec = Recorder(); rec.install()
    try:
        cap.start()                      # what _check_health schedules
    finally:
        rec.restore()
    left = [n for n in old if os.path.exists(os.path.join(cap.segment_dir, n))]
    shutil.rmtree(cap.segment_dir, ignore_errors=True)
    assert not left, f"segments from the previous capture survived a restart: {left}"


def test_a_failed_mix_falls_back_to_the_game_audio():
    """A failed amix used to lose the whole clip, or leave a partial WAV for
    -shortest to cut the video down to. The game audio is still there."""
    for how in ({"fail_on": "amix"}, {"timeout_on": "amix"}):
        audio = FakeAudio()
        audio.mic_available = True
        cap = make_capture(buffer_seconds=5, audio=audio)
        seed_segments(cap, 8)
        rec = Recorder(**how); rec.install()
        try:
            cap.save_replay()
        finally:
            rec.restore()
            shutil.rmtree(cap.segment_dir, ignore_errors=True)
        mux = [c for c in rec.run_cmds if "-c:a" in c]
        assert mux, f"a failed mix ({how}) cost the whole clip: no mux was run"
        cmd = mux[0]
        audio_in = os.path.basename(cmd[cmd.index("-i", cmd.index("-i") + 1) + 1])
        assert audio_in.startswith("loopback_"), \
            f"after a failed mix ({how}) the mux used {audio_in}, not the game audio"


def test_the_clip_is_anchored_on_capture_time_not_write_time():
    """A segment is written ~130 ms after it is captured at 240 fps (45 ms at
    60). Anchoring the clip on mtimes put the audio that far ahead of the
    picture, measured against NVIDIA Instant Replay. The window starts where
    the oldest selected segment was CAPTURED, its listed time used as is at
    every fps: taking out the 3 frames B-frames add to it measured worse."""
    for fps in (60, 240):
        audio = FakeAudio()
        cap = make_capture(fps=fps, buffer_seconds=5, audio=audio)
        lag, t0 = 0.130, 1000.0
        rec = Recorder(); rec.install()
        try:
            cap._start_ffmpeg()
            names = seed_segments(cap, 8)
            mtimes = {n: os.path.getmtime(os.path.join(cap.segment_dir, n)) for n in names}
            cap.proc.capture_t0 = t0
            with open(os.path.join(cap.segment_dir, cr.SEGMENT_LIST_FILE), "w") as f:
                for n in names[:-1]:                 # ffmpeg never lists the open one
                    start = mtimes[n] - lag - cr.SEGMENT_DURATION - t0
                    f.write(f"{n},{start:.6f},{start + cr.SEGMENT_DURATION:.6f}\n")
            cap.save_replay()
        finally:
            rec.restore()
            shutil.rmtree(cap.segment_dir, ignore_errors=True)
        oldest = f"seg_{int(rec.concat_marks[0][0][3:]):03d}.ts"
        _, t_end, duration = audio.save_calls[0]
        captured_open = mtimes[oldest] - lag - cr.SEGMENT_DURATION
        assert abs((t_end - duration) - captured_open) < 0.001, \
            f"at {fps} fps the audio window opens {((t_end - duration) - captured_open) * 1000:+.1f} ms " \
            f"away from when the oldest segment was captured"


def test_a_stale_list_row_never_anchors_the_clip():
    """ffmpeg replaces its segment list by renaming a .tmp over it, and a rename
    that meets a reader fails silently: the old rows stay, for names since
    reused, a whole wrap earlier. Trusted, one turned a save into a single
    segment under a buffer-long audio window."""
    audio = FakeAudio()
    cap = make_capture(buffer_seconds=5, audio=audio)
    rec = Recorder(); rec.install()
    try:
        cap._start_ffmpeg()
        names = seed_segments(cap, 8)
        cap.proc.capture_t0 = 0.0
        with open(os.path.join(cap.segment_dir, cr.SEGMENT_LIST_FILE), "w") as f:
            for n in names[:-1]:
                start = os.path.getmtime(os.path.join(cap.segment_dir, n)) - 9.0   # a wrap ago
                f.write(f"{n},{start:.6f},{start + 1:.6f}\n")
        cap.save_replay()
    finally:
        rec.restore()
        shutil.rmtree(cap.segment_dir, ignore_errors=True)
    _, _, duration = audio.save_calls[0]
    picked = len(rec.concat_marks[0])
    assert picked > 1 and duration < 5 + 2 * cr.SEGMENT_DURATION, \
        f"a stale row anchored the clip: {picked} segment(s) under {duration:.1f} s of audio"


def test_the_capture_clock_keeps_the_earliest_arrival():
    """The anchor is min(arrival - pts) over ffmpeg's -debug_ts capture lines:
    a line can only arrive late, never early. The min starts afresh every
    window, so it follows time.time() when Windows Time steps it forward."""
    line = (b"[vist#0:0/wrapped_avframe] demuxer+ffmpeg -> ist_index:0:0 "
            b"type:video pkt_pts:0 pkt_pts_time:%.3f pkt_dts:0\n")
    feed = [(100.05, 0.0), (101.02, 1.0), (101.80, 1.5),   # arrival - pts: .05 .02 .30
            (112.60, 12.0)]                                # a 0.58 s step forward
    proc, published = FakeProc(), []

    def pipe():
        for _, pts in feed:
            yield b"[out#0] muxer <- pkt_pts_time:9.0 must be ignored\n"
            yield line % pts
            published.append(getattr(proc, "capture_t0", None))

    arrivals = iter(now for now, _ in feed)
    real_time = cr.time.time
    cr.time.time = lambda: next(arrivals)
    try:
        cr.FFmpegCapture.__new__(cr.FFmpegCapture)._read_capture_clock(proc, pipe())
    finally:
        cr.time.time = real_time
    assert published[:2] == [None, None], f"published {published[:2]} before the first window closed"
    assert abs(published[2] - 100.02) < 1e-9, f"first window gave {published[2]}, expected its minimum 100.02"
    assert abs(published[3] - 100.60) < 1e-9,         f"after the clock stepped forward the anchor stayed at {published[3]}, expected 100.60"


def test_capture_feeds_its_clock_through_a_roomy_pipe():
    """The anchor needs -debug_ts on stderr, read by us, and the segment list.
    ffmpeg stalls, capture included, whenever that pipe is full, and -debug_ts
    writes ~1.5 KB a frame: Popen's own 4 KB pipe gave the reader ~11 ms at
    240 fps."""
    sizes = []
    real_create = cr._winapi.CreatePipe

    def create(attrs, size):
        sizes.append(size)
        return real_create(attrs, size)

    cap = make_capture(fps=240)
    rec = Recorder(); rec.install()
    cr._winapi.CreatePipe = create
    try:
        cap._start_ffmpeg()
    finally:
        cr._winapi.CreatePipe = real_create
        rec.restore()
        shutil.rmtree(cap.segment_dir, ignore_errors=True)
    cmd, kw = rec.popen_cmds[0], rec.popen_kwargs[0]
    assert "-debug_ts" in cmd and "-segment_list" in cmd, "the capture no longer produces its clock"
    assert kw["stderr"] >= 0, f"ffmpeg's stderr is {kw['stderr']}, not our pipe"   # PIPE -1, DEVNULL -3
    assert sizes and sizes[0] >= 1 << 20, f"stderr pipe of {sizes} bytes, want at least 1 MB"


class FakeStream:
    def __init__(self):
        self.active, self.closed = True, False

    def is_active(self):
        return self.active

    def stop_stream(self):
        pass

    def close(self):
        self.closed = True


def make_audio():
    a = cr.AudioCapture.__new__(cr.AudioCapture)   # skip __init__: no device probing
    a._loopback_device, a._mic_device = {"name": "lb"}, None
    a._loopback_stream, a._mic_stream = FakeStream(), None
    a._loopback_frames, a._mic_frames = 1000, 0
    a._running = True
    a._retry = {"loopback": [0, 0], "mic": [0, 0]}
    return a


def test_audio_heal_reopens_a_stream_that_dies_late():
    """The audio heal used to be a thread that stopped ~9 s after start(): a
    loopback that died hours in (sleep/resume, a driver reset) stayed dead,
    and 5 of 70 saves in one real log lost the game audio. heal() runs on
    every health tick, and never reopens a stream that is open, even one
    that has delivered nothing yet (silence)."""
    a = make_audio()
    a._mic_device, a._mic_stream = {"name": "mic"}, FakeStream()   # open, 0 frames
    first, mic, reopened = a._loopback_stream, a._mic_stream, []

    def reopen():
        reopened.append("loopback")
        return FakeStream()

    a._open_loopback_stream = reopen
    a._open_mic_stream = lambda: reopened.append("mic")
    for n in range(1, 15):
        if n == 10:                      # well past the old 6 x 1.5 s window
            first.active = False
        a.heal()
    assert reopened == ["loopback"], f"reopened {reopened}, expected the dead loopback once"
    assert first.closed and not mic.closed and a._loopback_stream is not first, \
        "an open stream was closed, or the dead one was kept"


def test_audio_heal_backs_off_then_leaves_a_gone_device_alone():
    """A device that stays gone, or opens and dies at once, is reopened 2, 4,
    8 ... ticks apart and then left alone: it can neither flood the capped log
    nor churn every tick. And it never delays the other stream, as one shared
    wait once let a gone mic hold a dead loopback back by up to a minute."""
    a = make_audio()
    a._mic_device = {"name": "mic"}
    first, now, mic_tries, lb_reopened = a._loopback_stream, [0], [], []

    def flap():                          # opens, but is dead by the next tick
        mic_tries.append(now[0])
        s = FakeStream()
        s.active = False
        return s

    def reopen():
        lb_reopened.append(now[0])
        return FakeStream()

    a._open_mic_stream, a._open_loopback_stream = flap, reopen
    for n in range(1, 301):
        now[0] = n
        if n == 50:
            first.active = False
        a.heal()
    gaps = [later - earlier for earlier, later in zip(mic_tries, mic_tries[1:])]
    assert gaps == [2, 4, 8, 16, 32], f"ticks between mic reopens: {gaps}"
    assert lb_reopened == [50], f"the dead loopback was reopened at ticks {lb_reopened}, not at once"


def test_audio_start_keeps_running_when_nothing_opens():
    """A first open can fail (sign-in, right after a device change). start()
    used to give up then, and nothing ever retried: heal() now does. It opens
    on the calling thread, never a worker: from a thread without COM, WASAPI
    failed every open with -9999."""
    a = make_audio()
    a._running, a._loopback_stream = False, None
    a._loopback_lock, a._mic_lock = threading.Lock(), threading.Lock()
    a._loopback_chunks, a._mic_chunks = [], []
    a._open_loopback_stream = a._open_mic_stream = lambda: None

    def no_thread(*_, **__):
        raise AssertionError("AudioCapture.start() spawned a thread")

    real_thread = cr.threading.Thread
    cr.threading.Thread = no_thread
    try:
        a.start()
    finally:
        cr.threading.Thread = real_thread
    assert a._running, "start() gave up when no stream opened, and nothing retries it"


def test_health_check_heals_audio_and_survives_a_failing_heal():
    """_check_health is the tk thread's 2 s tick, and the tk thread is the
    one with COM. It reschedules before healing, so one failing heal cannot
    end the chain that also restarts a dead ffmpeg."""
    calls = []

    class Audio:
        def heal(self):
            calls.append("heal")
            raise RuntimeError("boom")

    cap = make_capture(audio=Audio())
    cap.proc = FakeProc()
    try:
        cap._check_health()
    finally:
        shutil.rmtree(cap.segment_dir, ignore_errors=True)
    assert calls == ["heal"], "the health tick does not heal the audio"
    assert cap._poll_id is not None, "a failing heal ended the health chain"


def test_ffmpeg_download_refuses_an_unpinned_binary():
    """ffmpeg.exe ships inside the installer, the only release asset. A download
    that does not hash to the pinned build must never reach the disk, or one
    swapped file upstream would ship to every user."""
    import contextlib
    import hashlib
    import io
    import urllib.request
    import zipfile

    payload = b"not ffmpeg"
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        zf.writestr("ffmpeg-x/bin/ffmpeg.exe", payload)

    spec = importlib.util.spec_from_file_location(
        "download_ffmpeg", os.path.join(REPO_DIR, "scripts", "download_ffmpeg.py"))
    dl = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(dl)
    d = tempfile.mkdtemp(prefix="inv_test_")
    dl.EXE = os.path.join(d, "ffmpeg.exe")       # never the repo's real ffmpeg.exe

    def run():
        """main()'s exit code, output silenced. SystemExit must never escape:
        this suite's loop only catches Exception, so one would end the whole
        run with no FAIL line and no summary."""
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                dl.main()
            return 0
        except SystemExit as e:
            return e.code

    real_urlopen = urllib.request.urlopen
    urllib.request.urlopen = lambda *a, **k: io.BytesIO(buf.getvalue())
    try:
        refused = run() == 1
        written_unverified = os.path.exists(dl.EXE)
        with open(dl.EXE, "wb") as f:            # a stale build already in place
            f.write(b"stale")
        refused_again = run() == 1
        stale_kept = os.path.exists(dl.EXE)
        dl.FFMPEG_SHA256 = hashlib.sha256(payload).hexdigest()
        accepted = run() == 0
        with open(dl.EXE, "rb") as f:
            written_when_pinned = f.read() == payload

        def no_network(*a, **k):                 # the verified copy must be kept as is
            raise AssertionError("a verified ffmpeg.exe was deleted and fetched again")
        urllib.request.urlopen = no_network
        kept_verified = run() == 0 and os.path.exists(dl.EXE)
    finally:
        urllib.request.urlopen = real_urlopen
        shutil.rmtree(d, ignore_errors=True)
    assert refused and refused_again, \
        "a download that does not match the pinned hash was not refused"
    assert not written_unverified, "an unverified ffmpeg.exe was written to disk"
    assert not stale_kept, \
        "a stale ffmpeg.exe survived a refused download, for the next build to bundle"
    assert accepted and written_when_pinned, \
        "a download matching the pinned hash was not written"
    assert kept_verified, "an already verified ffmpeg.exe was not kept"


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
