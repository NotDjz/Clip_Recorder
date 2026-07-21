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

REPO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP = os.path.join(REPO_DIR, "src", "clip_recorder.pyw")
SUITE = os.path.join(REPO_DIR, "tests", "test_invariants.py")

# (label, find, replace, the test that should catch it)
MUTATIONS = [
    ("include the in-progress segment",
     "complete = files_with_mtime[:-1]", "complete = files_with_mtime[:]",
     "test_in_progress_segment_is_never_selected"),
    ("anchor the fallback on mtime (the 1s-offset bug)",
     "total_duration = save_time - (complete[0][1] - SEGMENT_DURATION)",
     "total_duration = save_time - complete[0][1]",
     "test_fallback_anchors_at_the_oldest_segment_open_time"),
    ("restart audio on a video restart",
     "        self._stop_ffmpeg()\n        self._wipe_segments()\n        self._start_ffmpeg()",
     "        self.stop()\n        self._wipe_segments()\n        self.start()",
     "test_restart_video_does_not_touch_audio"),
    ("decouple keyframes from the segment length",
     "keyframe_interval = fps * SEGMENT_DURATION", "keyframe_interval = fps * 5",
     "test_keyframe_interval_equals_fps_times_segment_duration"),
    ("wipe every temp file, including a running save's",
     'if not (f.startswith("seg_") and f.endswith(".ts")):\n                continue',
     'if False:\n                continue',
     "test_wipe_segments_spares_an_in_flight_save"),
    ("seek into a segment instead of taking whole ones",
     '"-ss", "0",', '"-ss", "0.5",',
     "test_never_seeks_into_a_segment"),
    ("log from inside the realtime audio callback",
     "        self._loopback_status_flags |= status",
     "        log('cb')\n        self._loopback_status_flags |= status",
     "test_audio_callbacks_do_no_blocking_io"),
]


def run_suite():
    """Names of the tests that failed (lines look like '  FAIL  name: msg')."""
    p = subprocess.run([sys.executable, SUITE], capture_output=True, text=True)
    return {l.split()[1].rstrip(":") for l in p.stdout.splitlines()
            if l.strip().startswith(("FAIL", "ERROR"))}


def main():
    original = io.open(APP, encoding="utf-8", newline="").read()
    orig_hash = hashlib.sha256(original.encode()).hexdigest()

    base_fail = run_suite()
    print(f"baseline: {'clean' if not base_fail else 'ALREADY FAILING: ' + str(base_fail)}\n")
    ok = not base_fail

    for label, find, repl, expect in MUTATIONS:
        if find not in original:
            print(f"  SKIP    {label}\n          anchor no longer in the source — update this mutation")
            ok = False
            continue
        io.open(APP, "w", encoding="utf-8", newline="").write(original.replace(find, repl, 1))
        try:
            failing = run_suite()
        finally:
            io.open(APP, "w", encoding="utf-8", newline="").write(original)
        ok &= bool(failing)
        named = expect in failing
        print(f"  {'CAUGHT' if failing else 'MISSED'}  {label}")
        if not failing:
            print("          suite stayed GREEN — this invariant is not guarded")
        elif not named:
            print(f"          caught by {', '.join(sorted(failing))} (expected {expect})")

    restored = hashlib.sha256(
        io.open(APP, encoding="utf-8", newline="").read().encode()).hexdigest() == orig_hash
    print(f"\nsource restored intact: {restored}")
    print("RESULT:", "PASS — every invariant is genuinely guarded"
          if ok and restored else "PROBLEM — see above")
    raise SystemExit(0 if ok and restored else 1)


if __name__ == "__main__":
    main()
