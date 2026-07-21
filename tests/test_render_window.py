"""
Deterministic unit tests for AudioCapture._render_window — the real-time audio
reconstruction that anchors loopback + mic to wall-clock time (padding silence
for gaps that WASAPI loopback leaves when the source goes quiet).

No capture, no devices, no ffmpeg — pure logic. Run:
    py tests\\test_render_window.py
"""
import array
import importlib.util
import os

REPO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP_PATH = os.path.join(REPO_DIR, "src", "clip_recorder.pyw")

spec = importlib.util.spec_from_file_location("clip_recorder", APP_PATH)
cr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cr)

RW = cr.AudioCapture._render_window
RATE, CH, SW = 48000, 1, 2


def chunk(val, nframes):
    return array.array("h", [val] * nframes).tobytes()


def sample_at(pcm, t_since_window_start):
    idx = int(round(t_since_window_start * RATE))
    b = idx * CH * SW
    return array.array("h", pcm[b:b + 2])[0]


def test_silence_gap_preserved():
    # chunk1 covers real [0.5,1.0] val=100; 3s gap; chunk2 covers [4.0,4.5] val=200
    chunks = [(1.0, chunk(100, RATE // 2)), (4.5, chunk(200, RATE // 2))]
    out = RW(chunks, RATE, CH, SW, 5.0, 5.0)
    assert out is not None and len(out) // 2 == 5 * RATE
    for t, exp in {0.25: 0, 0.75: 100, 2.5: 0, 4.25: 200, 4.75: 0}.items():
        assert sample_at(out, t) == exp, f"t={t} exp {exp} got {sample_at(out, t)}"


def test_window_clipping():
    chunks = [(1.0, chunk(100, RATE // 2)), (4.5, chunk(200, RATE // 2))]
    out = RW(chunks, RATE, CH, SW, 4.5, 1.0)  # window [3.5,4.5]
    assert len(out) // 2 == RATE
    assert sample_at(out, 0.25) == 0      # 3.75s: silence
    assert sample_at(out, 0.75) == 200    # 4.25s: chunk2


def test_dead_stream_returns_none():
    assert RW([], RATE, CH, SW, 5.0, 5.0) is None


def test_out_of_window_excluded():
    assert RW([(0.5, chunk(100, RATE // 2))], RATE, CH, SW, 5.0, 1.0) is None


def test_jittered_arrivals_stay_contiguous():
    """Regression test for the crackling: real callback arrival times jitter by
    a few ms. A continuous run must be stitched sample-contiguously — no silence
    holes punched at the chunk boundaries (that was ~12 clicks/second)."""
    n = RATE // 10                      # 0.1 s chunks
    jitter = [0.0, +0.004, -0.003, +0.005, -0.002, +0.003, -0.004, +0.002, 0.0, -0.001]
    chunks = [((k + 1) / 10.0 + jitter[k], chunk(7, n)) for k in range(10)]
    out = RW(chunks, RATE, CH, SW, 1.0 + jitter[-1], 1.0)
    assert len(out) // 2 == RATE
    vals = array.array("h", out)
    # Interior must be solid: not a single zero sample punched into the run.
    holes = sum(1 for v in vals[n // 2: RATE - n // 2] if v == 0)
    assert holes == 0, f"{holes} silence samples punched into a continuous run"


def test_contiguous_reconstruct():
    ce = [(t / 10.0, chunk(t, RATE // 10)) for t in range(1, 11)]  # 10 x 0.1s, vals 1..10
    out = RW(ce, RATE, CH, SW, 1.0, 1.0)
    assert len(out) // 2 == RATE
    for k in range(1, 11):
        t = (k - 0.5) * 0.1
        assert sample_at(out, t) == k, f"chunk {k} t={t:.2f} got {sample_at(out, t)}"


def main():
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for t in tests:
        t()
        print(f"{t.__name__}: PASS")
    print(f"\nALL {len(tests)} _render_window TESTS PASSED")


if __name__ == "__main__":
    main()
