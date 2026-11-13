//! The capture clock: ties ffmpeg's pts to the wall clock the audio is stamped with.
//!
//! A -debug_ts `demuxer+ffmpeg ->` line is printed as its frame leaves the capture and can only
//! arrive late, never early, so min(arrival - pts) is the clock's zero (p50 spread 0.1 ms,
//! measured). The minimum starts afresh every window: Windows Time slews and steps the wall
//! clock against the QPC behind ddagrab's pts, and a minimum kept forever could never follow a
//! forward step. Placing clips by this clock instead of segment mtimes removed an A/V offset
//! that grew with fps (-105 ms at 240 fps).

/// Wall-clock seconds, the one clock for audio arrivals, the capture clock, save times and file
/// mtimes (Rust's SystemTime is GetSystemTimePreciseAsFileTime on Windows).
pub fn now() -> f64 {
    seconds(std::time::SystemTime::now())
}

pub fn seconds(t: std::time::SystemTime) -> f64 {
    t.duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// First estimate after this long, so a save soon after start has a clock.
const FIRST_WINDOW_S: f64 = 1.0;
const WINDOW_S: f64 = 10.0;

/// The pts of a capture line, or None for any other line.
pub fn capture_pts(line: &[u8]) -> Option<f64> {
    let line = std::str::from_utf8(line).ok()?;
    if !line.contains("demuxer+ffmpeg ->") {
        return None;
    }
    let (_, rest) = line.split_once("pkt_pts_time:")?;
    rest.split_whitespace().next()?.parse().ok()
}

#[derive(Debug, Default)]
pub struct Estimator {
    best: Option<f64>,
    window_end: Option<f64>,
}

impl Estimator {
    /// Feed one capture line's arrival time and pts. Returns the clock's zero when a window
    /// closes, to publish; None otherwise.
    pub fn observe(&mut self, arrival: f64, pts: f64) -> Option<f64> {
        let zero = arrival - pts;
        self.best = Some(self.best.map_or(zero, |b| b.min(zero)));
        match self.window_end {
            None => {
                self.window_end = Some(arrival + FIRST_WINDOW_S);
                None
            }
            Some(end) if arrival >= end => {
                self.window_end = Some(arrival + WINDOW_S);
                self.best.take()
            }
            Some(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// After the first 1 s window, each window lasts 10 s: a clock published on every line would
    /// follow each line's scheduling jitter instead of the earliest arrival.
    #[test]
    fn later_windows_last_ten_seconds() {
        let mut e = Estimator::default();
        assert_eq!(e.observe(100.0, 0.0), None, "opens the first window");
        assert!(e.observe(101.0, 1.0).is_some(), "the first window closes after 1 s");
        assert_eq!(e.observe(102.0, 2.0), None, "the next one is still open");
        assert_eq!(e.observe(110.9, 10.9), None);
        assert!(e.observe(111.0, 11.0).is_some(), "and closes 10 s after it opened");
    }

    #[test]
    fn only_capture_lines_carry_the_clock() {
        let capture = b"[vist#0:0/wrapped_avframe] demuxer+ffmpeg -> ist_index:0:0 type:video pkt_pts:0 pkt_pts_time:1.5 pkt_dts:0";
        assert_eq!(capture_pts(capture), Some(1.5));
        assert_eq!(capture_pts(b"[out#0] muxer <- pkt_pts_time:9.0 must be ignored"), None);
        assert_eq!(capture_pts(b"demuxer+ffmpeg -> pkt_pts_time:garbage"), None);
        assert_eq!(capture_pts(b"demuxer+ffmpeg -> no pts here"), None);
    }

    #[test]
    fn the_clock_keeps_each_window_s_earliest_arrival_and_follows_a_step() {
        let mut e = Estimator::default();
        // arrival - pts: .05, .02, .30, then a 0.58 s forward step of the wall clock.
        let feed = [(100.05, 0.0), (101.02, 1.0), (101.80, 1.5), (112.60, 12.0)];
        let published: Vec<Option<f64>> = feed.iter().map(|&(t, pts)| e.observe(t, pts)).collect();
        assert_eq!(published[..2], [None, None], "published before the first window closed");
        assert!(
            (published[2].unwrap() - 100.02).abs() < 1e-9,
            "first window gave {:?}",
            published[2]
        );
        assert!(
            (published[3].unwrap() - 100.60).abs() < 1e-9,
            "did not follow the step: {:?}",
            published[3]
        );
    }
}
