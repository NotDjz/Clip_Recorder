//! Rebuilds an audio window on the real wall-clock timeline from timestamped chunks.
//!
//! WASAPI loopback does not deliver continuously: it skips silence. Counting samples therefore
//! put loopback and mic on different timelines, and the mix echoed (the long-running "audio is
//! seconds off" bug). Each chunk is placed at its real arrival time instead, gaps stay silent,
//! and loopback, mic and video share one timeline by construction.
//!
//! Callback arrivals jitter by a few ms, though, and placing every chunk at its own timestamp
//! punched a 1-5 ms hole at each boundary: ~12 clicks a second. A run of chunks that arrives
//! roughly on schedule is therefore kept sample-contiguous; only a divergence over TOLERANCE_S
//! (a real gap) re-anchors on the timestamp. Comparing with the absolute real position each
//! time keeps the run from drifting.

/// Divergence from the real position that counts as a gap rather than jitter.
pub const TOLERANCE_S: f64 = 0.030;

/// One callback's samples (interleaved int16), which end at `arrival` (wall-clock seconds).
pub struct Chunk<'a> {
    pub arrival: f64,
    pub samples: &'a [i16],
}

/// Exactly round(duration * rate) frames ending at `end`, or None if no chunk overlaps the
/// window (a dead stream: the clip then falls back to the other source).
pub fn render_window(chunks: &[Chunk], rate: u32, channels: u16, end: f64, duration: f64) -> Option<Vec<i16>> {
    let channels = channels.max(1) as usize;
    let total = (duration * rate as f64).round() as i64;
    if total <= 0 {
        return None;
    }
    let mut out = vec![0i16; total as usize * channels];
    let start = end - duration;
    let tolerance = (rate as f64 * TOLERANCE_S) as i64;
    let mut cursor: Option<i64> = None; // where a continuing run would land next
    let mut wrote = false;
    for chunk in chunks {
        let frames = (chunk.samples.len() / channels) as i64;
        if frames == 0 {
            continue;
        }
        // The chunk ends at its arrival.
        let real = ((chunk.arrival - start) * rate as f64).round() as i64 - frames;
        let place = match cursor {
            Some(c) if (real - c).abs() <= tolerance => c,
            _ => real,
        };
        cursor = Some(place + frames);
        let skip = (-place).max(0);
        let dst = place.max(0);
        let count = (frames - skip).min(total - dst);
        if count <= 0 {
            continue;
        }
        let (from, to) = ((skip as usize) * channels, (dst as usize) * channels);
        let n = count as usize * channels;
        out[to..to + n].copy_from_slice(&chunk.samples[from..from + n]);
        wrote = true;
    }
    wrote.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48000;

    fn chunk(value: i16, frames: usize) -> Vec<i16> {
        vec![value; frames]
    }

    fn at(out: &[i16], t: f64) -> i16 {
        out[(t * RATE as f64).round() as usize]
    }

    fn render(chunks: &[(f64, Vec<i16>)], end: f64, duration: f64) -> Option<Vec<i16>> {
        let chunks: Vec<Chunk> = chunks.iter().map(|(t, s)| Chunk { arrival: *t, samples: s }).collect();
        render_window(&chunks, RATE, 1, end, duration)
    }

    #[test]
    fn a_silence_gap_stays_silent_where_it_really_was() {
        // [0.5, 1.0] holds 100, a 3 s gap, [4.0, 4.5] holds 200.
        let chunks = [(1.0, chunk(100, 24000)), (4.5, chunk(200, 24000))];
        let out = render(&chunks, 5.0, 5.0).unwrap();
        assert_eq!(out.len(), 5 * RATE as usize);
        for (t, expected) in [(0.25, 0), (0.75, 100), (2.5, 0), (4.25, 200), (4.75, 0)] {
            assert_eq!(at(&out, t), expected, "at {t} s");
        }
    }

    #[test]
    fn the_window_clips_what_is_outside_it() {
        let chunks = [(1.0, chunk(100, 24000)), (4.5, chunk(200, 24000))];
        let out = render(&chunks, 4.5, 1.0).unwrap(); // window [3.5, 4.5]
        assert_eq!(out.len(), RATE as usize);
        assert_eq!((at(&out, 0.25), at(&out, 0.75)), (0, 200));
    }

    #[test]
    fn a_dead_stream_gives_nothing() {
        assert_eq!(render(&[], 5.0, 5.0), None);
    }

    #[test]
    fn audio_outside_the_window_gives_nothing() {
        assert_eq!(render(&[(0.5, chunk(100, 24000))], 5.0, 1.0), None);
    }

    #[test]
    fn jittered_arrivals_stay_contiguous() {
        // The crackling regression: not a single silent sample may be punched into a run.
        let n = RATE as usize / 10;
        let jitter = [0.0, 0.004, -0.003, 0.005, -0.002, 0.003, -0.004, 0.002, 0.0, -0.001];
        let chunks: Vec<(f64, Vec<i16>)> = (0..10).map(|k| ((k + 1) as f64 / 10.0 + jitter[k], chunk(7, n))).collect();
        let out = render(&chunks, 1.0 + jitter[9], 1.0).unwrap();
        assert_eq!(out.len(), RATE as usize);
        let holes = out[n / 2..RATE as usize - n / 2].iter().filter(|&&v| v == 0).count();
        assert_eq!(holes, 0, "{holes} silent samples punched into a continuous run");
    }

    #[test]
    fn contiguous_chunks_land_in_their_slots() {
        let chunks: Vec<(f64, Vec<i16>)> = (1..=10).map(|t| (t as f64 / 10.0, chunk(t as i16, RATE as usize / 10))).collect();
        let out = render(&chunks, 1.0, 1.0).unwrap();
        for k in 1..=10 {
            assert_eq!(at(&out, (k as f64 - 0.5) * 0.1), k as i16, "chunk {k}");
        }
    }

    #[test]
    fn stereo_frames_are_placed_as_frames() {
        // 0.5 s of stereo frames ending at 1.0 in a [0, 1] window: the second half, both channels.
        let samples = vec![9i16; 2 * 24000];
        let out = render_window(&[Chunk { arrival: 1.0, samples: &samples }], RATE, 2, 1.0, 1.0).unwrap();
        assert_eq!(out.len(), 2 * RATE as usize);
        assert_eq!((out[2 * 12000], out[2 * 36000], out[2 * 36000 + 1]), (0, 9, 9));
    }
}
