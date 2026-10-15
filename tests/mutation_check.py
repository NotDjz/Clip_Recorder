"""
Proves the guard rails in test_invariants.py actually bite.

A suite that stays green no matter what is worse than no suite: it buys false
confidence. This breaks each invariant on purpose, one at a time, and checks the
suite goes red. Two of these tests were silently useless until this caught them
(one matched on filenames that snapshotting had already renamed; one only ever
exercised the no-audio branch).

The source is restored after every mutation and its hash verified at the end, so
the working tree is left exactly as it was found.

    py tests\\mutation_check.py
"""
import hashlib
import io
import os
import subprocess
import sys

CR, LF = chr(13), chr(10)

REPO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP = os.path.join(REPO_DIR, "src", "clip_recorder.pyw")
DOWNLOADER = os.path.join(REPO_DIR, "scripts", "download_ffmpeg.py")
SUITE = os.path.join(REPO_DIR, "tests", "test_invariants.py")

# (label, find, replace, the test that should catch it[, file, default APP])
MUTATIONS = [
    ("write a downloaded ffmpeg.exe without checking the pin",
     "hashlib.sha256(exe).hexdigest() != FFMPEG_SHA256", "False",
     "test_ffmpeg_download_refuses_an_unpinned_binary", DOWNLOADER),
    ("re-fetch (or lose) an ffmpeg.exe that is already verified",
     "hexdigest() == FFMPEG_SHA256:", "hexdigest() != FFMPEG_SHA256:",
     "test_ffmpeg_download_refuses_an_unpinned_binary", DOWNLOADER),
    ("leave a stale ffmpeg.exe in place for the next build",
     "os.remove(EXE)", "pass",
     "test_ffmpeg_download_refuses_an_unpinned_binary", DOWNLOADER),
    ("include the in-progress segment",
     "complete = files_with_mtime[:-1]", "complete = files_with_mtime[:]",
     "test_in_progress_segment_is_never_selected"),
    ("anchor the fallback on mtime (the 1s-offset bug)",
     "else complete[0][1] - SEGMENT_DURATION", "else complete[0][1]",
     "test_fallback_anchors_at_the_oldest_segment_open_time"),
    ("anchor the clip on write time again (audio ~130 ms early at 240 fps)",
     "return t if t is not None and", "return mtime_open(i) if t is not None and",
     "test_the_clip_is_anchored_on_capture_time_not_write_time"),
    ("take the latest arrival as the capture clock's origin",
     "if best is None or t0 < best:", "if best is None or t0 > best:",
     "test_the_capture_clock_keeps_the_earliest_arrival"),
    ("keep the capture clock's min forever (never follows a clock step)",
     "best, window_end = None, now + 10", "window_end = now + 10",
     "test_the_capture_clock_keeps_the_earliest_arrival"),
    ("give ffmpeg's stderr Popen's 4 KB pipe (a busy reader stalls the capture)",
     "_winapi.CreatePipe(None, 1 << 20)", "_winapi.CreatePipe(None, 0)",
     "test_capture_feeds_its_clock_through_a_roomy_pipe"),
    ("stop printing the capture clock (every clip back on mtimes)",
     'cmd = [FFMPEG, "-y", "-debug_ts"]', 'cmd = [FFMPEG, "-y"]',
     "test_capture_feeds_its_clock_through_a_roomy_pipe"),
    ("throw ffmpeg's stderr away (the reader never sees a line)",
     "stderr=w,", "stderr=subprocess.DEVNULL,",
     "test_capture_feeds_its_clock_through_a_roomy_pipe"),
    ("take the B-frame shift out of the segment list (measured 30 ms worse)",
     "t0 + float(start) for name", "t0 + float(start) - 0.0125 for name",
     "test_the_clip_is_anchored_on_capture_time_not_write_time"),
    ("trust a stale segment-list row (a whole wrap off)",
     "t > mtime - 2 * SEGMENT_DURATION", "True",
     "test_a_stale_list_row_never_anchors_the_clip"),
    ("stop healing the audio from the health tick (a late death stays dead)",
     "self.audio.heal()", "pass",
     "test_health_check_heals_audio_and_survives_a_failing_heal"),
    ("reopen a gone device every tick (churns, floods the capped log)",
     "retry[0] = 2 ** retry[1] - 1", "retry[0] = 0",
     "test_audio_heal_backs_off_then_leaves_a_gone_device_alone"),
    ("never leave a gone device alone (a reopen a minute, forever)",
     "if retry[1] >= HEAL_MAX_REOPENS:", "if False:",
     "test_audio_heal_backs_off_then_leaves_a_gone_device_alone"),
    ("share one wait between both streams (a gone mic delays the loopback)",
     "retry = self._retry[kind]", "retry = self._retry['loopback']",
     "test_audio_heal_backs_off_then_leaves_a_gone_device_alone"),
    ("reopen an open stream that has delivered nothing yet (silence)",
     "if alive:", "if False:",
     "test_audio_heal_reopens_a_stream_that_dies_late"),
    ("keep the old capture's segment list across a restart",
     " and f != SEGMENT_LIST_FILE:", ":",
     "test_a_restart_never_mixes_two_captures"),
    ("restart audio on a video restart",
     "        self._stop_ffmpeg()\n        self._start_ffmpeg()",
     "        self.stop()\n        self.start()",
     "test_restart_video_does_not_touch_audio"),
    ("keep a dead capture's segments across a restart",
     "self._wipe_segments()", "pass",
     "test_a_restart_never_mixes_two_captures"),
    ("let a mix that times out lose the clip",
     "mix_ok = False", "raise",
     "test_a_failed_mix_falls_back_to_the_game_audio"),
    ("trust a failed mix's partial WAV",
     "mux_audio = loopback_wav", "pass",
     "test_a_failed_mix_falls_back_to_the_game_audio"),
    ("decouple keyframes from the segment length",
     "keyframe_interval = fps * SEGMENT_DURATION", "keyframe_interval = fps * 5",
     "test_keyframe_interval_equals_fps_times_segment_duration"),
    ("wipe every temp file, including a running save's",
     'if not (f.startswith("seg_") and f.endswith(".ts")) and f != SEGMENT_LIST_FILE:',
     'if False:',
     "test_wipe_segments_spares_an_in_flight_save"),
    ("let NVENC hold B-frame output back (video lags the audio)",
     '"-bf", "3", "-delay", "0"', '"-bf", "3"',
     "test_nvenc_b_frames_never_hold_output_back"),
    ("use B-frames at fps where their lag was never measured",
     "if fps >= B_FRAMES_MIN_FPS:", "if True:",
     "test_nvenc_b_frames_never_hold_output_back"),
    ("bring back -ss 0, which drops the first B-frame segment",
     '"-t", str(total_duration),', '"-ss", "0", "-t", str(total_duration),',
     "test_never_seeks_into_a_segment"),
    ("log from inside the realtime audio callback",
     "        self._loopback_status_flags |= status",
     "        log('cb')\n        self._loopback_status_flags |= status",
     "test_audio_callbacks_do_no_blocking_io"),
    ("stop recording the ffmpeg PID (orphans become unfindable)",
     'with open(os.path.join(self.segment_dir, FFMPEG_PID_FILE), "w") as f:\n                f.write(str(self.proc.pid))',
     "pass",
     "test_capture_records_its_ffmpeg_pid"),
    ("kill the recorded PID without checking it is still ffmpeg",
     'and os.path.basename(buf.value).lower() == "ffmpeg.exe"):',
     "and True):",
     "test_orphan_cleanup_never_kills_a_foreign_process"),
    ("let a missing or garbage PID file raise instead of being tolerated",
     "    try:\n        with open(os.path.join(seg_dir, FFMPEG_PID_FILE)) as f:\n"
     "            pid = int(f.read().strip())\n    except Exception:\n        return",
     "    with open(os.path.join(seg_dir, FFMPEG_PID_FILE)) as f:\n"
     "        pid = int(f.read().strip())",
     "test_orphan_cleanup_tolerates_a_missing_or_bogus_pid_file"),
    ("neuter the job's kill-on-close flag (job created, does nothing)",
     "JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000",
     "JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00000000",
     "test_job_object_is_kill_on_close_with_explicit_breakaway"),
    ("spawn ffmpeg before joining the job (children escape it)",
     "        _ensure_kill_on_close_job()\n        self.segment_dir",
     "        self.segment_dir",
     "test_capture_joins_the_job_before_spawning_ffmpeg"),
    # Both anchors below are deliberately short. A long anchor that quotes a log
    # message or four consecutive statements dies on any unrelated edit, and a
    # dead anchor degrades to SKIP — a guard rail that stops being checked.
    ("change the monitor label separator (device selection breaks silently)",
     "f\"{i+1}: {m['name']}",
     "f\"{i+1} - {m['name']}",
     "test_settings_vars_match_what_apply_parses"),
    ("forget the Startup shortcut on uninstall (Windows retries a dead exe forever)",
     '"CR_STARTUP_LNK": startup_lnk',
     '"CR_STARTUP_LNK": desktop_lnk',
     "test_uninstall_removes_known_files_and_never_recurses"),
    ("let the banner be recorded into the clip (drop EXCLUDEFROMCAPTURE)",
     "SetWindowDisplayAffinity(hwnd, 0x00000011)",
     "SetWindowDisplayAffinity(hwnd, 0x00000000)",
     "test_banner_keeps_its_window_styles"),
    ("guard the whole orphan sweep instead of each directory",
     "            try:\n                _kill_orphan_ffmpeg(path, deadline)",
     "            _kill_orphan_ffmpeg(path, deadline)\n            try:\n                pass",
     "test_orphan_sweep_isolates_each_directory"),
    ("probe ddagrab before joining the job (the one-probe blind spot)",
     "        _ensure_kill_on_close_job()",
     "        self.has_ddagrab = detect_ddagrab()\n        _ensure_kill_on_close_job()",
     "test_capture_joins_the_job_before_spawning_ffmpeg"),
    ("let the job kill the uninstall helper (uninstall silently does nothing)",
     "creationflags=0x08000000 | CREATE_BREAKAWAY_FROM_JOB,",
     "creationflags=0x08000000,",
     "test_uninstall_helper_breaks_away_from_the_job"),
    ("wipe the PID file along with the segments",
     'if not (f.startswith("seg_") and f.endswith(".ts")) and f != SEGMENT_LIST_FILE:',
     'if f == FFMPEG_PID_FILE:\n                os.remove(os.path.join(self.segment_dir, f))\n                continue\n'
     '            if not (f.startswith("seg_") and f.endswith(".ts")) and f != SEGMENT_LIST_FILE:',
     "test_wipe_segments_keeps_the_pid_file"),
    ("play a chime inline, stalling the thread that asked for it",
     "    threading.Thread(target=run, daemon=True).start()", "    run()",
     "test_chimes_never_play_on_the_calling_thread"),
    ("let a lost clip go unheard (the ascent never resolves)",
     "        play_fail_sound()", "        pass",
     "test_a_lost_clip_is_heard_as_well_as_seen"),
    ("stop acknowledging the press (no chime at all)",
     "        play_start_sound()", "        pass",
     "test_the_hotkey_acknowledges_the_press"),
    ("stop reporting a failed encode (the worker's own give-up path)",
     "                if not success:", "                if False:",
     "test_a_failed_encode_reports_instead_of_going_quiet"),
    # The mutations below guard the "Saving clip" banner and its chimes.
    ("call a too-soon press a lost clip (capture only just restarted)",
     '_fail("too_soon" if age < SEGMENT_DURATION * 3 else "error")',
     '_fail("error")',
     "test_giving_up_reports_a_reason"),
    ("stop telling anyone the save gave up too soon",
     '_fail("too_soon")', "pass",
     "test_giving_up_reports_a_reason"),
    ("stop reporting a save whose segment directory vanished",
     '_fail("error")', "pass",
     "test_giving_up_reports_a_reason"),
    ("stop retiring the generation when a banner is replaced",
     "self._pulse_gen += 1", "pass",
     "test_a_superseded_banner_stops_pulsing"),
    ("let a replaced banner keep pulsing its successor's dot",
     "gen != self._pulse_gen or ", "",
     "test_a_superseded_banner_stops_pulsing"),
]


def fit_endings(text, src):
    """Anchors here are written with \n line breaks, but the working tree is
    CRLF whenever git checked it out under core.autocrlf - the normal state on
    Windows. The source is read with newline='' to keep its bytes intact for the
    hash check, so every multi-line anchor silently stops matching and degrades
    to SKIP: a guard rail that has quietly stopped being checked, which is the
    exact failure this file exists to catch. Seen for real after a git stash.
    """
    if (CR + LF) in src:
        return text.replace(CR + LF, LF).replace(LF, CR + LF)
    return text


def run_suite():
    """Names of the tests that failed (lines look like '  FAIL  name: msg').

    Matched on the suite's exact prefix, never on a stripped line: code under
    test may print its own "ERROR: ..." and must not read as a failing test."""
    p = subprocess.run([sys.executable, SUITE], capture_output=True, text=True)
    failing = {l.split()[1].rstrip(":") for l in p.stdout.splitlines()
               if l.startswith(("  FAIL  ", "  ERROR "))}
    if p.returncode != 0 and not failing:
        # It died before printing a result line (an escaped SystemExit, an
        # import error): that is not a clean run, whatever stdout says.
        failing.add("<suite crashed>")
    return failing


def read(path):
    return io.open(path, encoding="utf-8", newline="").read()


def main():
    files = {APP} | {m[4] for m in MUTATIONS if len(m) > 4}
    originals = {p: read(p) for p in files}
    orig_hashes = {p: hashlib.sha256(originals[p].encode()).hexdigest() for p in files}

    base_fail = run_suite()
    print(f"baseline: {'clean' if not base_fail else 'ALREADY FAILING: ' + str(base_fail)}\n")
    ok = not base_fail

    for label, find, repl, expect, *target in MUTATIONS:
        path = target[0] if target else APP
        original = originals[path]
        find = fit_endings(find, original)
        repl = fit_endings(repl, original)
        if find not in original:
            print(f"  SKIP    {label}\n          anchor no longer in the source — update this mutation")
            ok = False
            continue
        io.open(path, "w", encoding="utf-8", newline="").write(original.replace(find, repl, 1))
        try:
            failing = run_suite()
        finally:
            io.open(path, "w", encoding="utf-8", newline="").write(original)
        ok &= bool(failing)
        named = expect in failing
        print(f"  {'CAUGHT' if failing else 'MISSED'}  {label}")
        if not failing:
            print("          suite stayed GREEN — this invariant is not guarded")
        elif not named:
            print(f"          caught by {', '.join(sorted(failing))} (expected {expect})")

    restored = all(hashlib.sha256(read(p).encode()).hexdigest() == orig_hashes[p]
                   for p in files)
    print(f"\nsource restored intact: {restored}")
    print("RESULT:", "PASS — every invariant is genuinely guarded"
          if ok and restored else "PROBLEM — see above")
    raise SystemExit(0 if ok and restored else 1)


if __name__ == "__main__":
    main()
