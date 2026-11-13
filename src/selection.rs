//! Which segments make a clip, and where its audio window starts.
//!
//! Whole segments only (a mid-segment seek once cost a segment per clip with B-frames), never
//! the newest one (ffmpeg is still writing it: torn frames), walking back from the newest until
//! the span covers the requested length. The window starts where the oldest chosen segment was
//! CAPTURED, per ffmpeg's segment list and the capture clock; mtimes, when the write happened,
//! trail the capture by 30 to 230 ms depending on fps, and anchoring on them put the sound ahead
//! of the picture at high fps.

use crate::ffmpeg_cmd::SEGMENT_DURATION;
use std::collections::HashMap;

/// A press this soon after the capture (re)started is "not enough footage yet", not a failure.
pub const TOO_SOON_S: f64 = SEGMENT_DURATION as f64 * 3.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub name: String,
    /// Last write, wall-clock seconds.
    pub mtime: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GiveUp {
    /// Not enough footage yet: the capture only just (re)started.
    TooSoon,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    /// Oldest first.
    pub segments: Vec<Segment>,
    /// From the window's start to the save: the video's -t and the audio window's length.
    pub duration: f64,
    pub window_start: f64,
    /// True when the start came from the capture clock, false when it fell back to mtimes.
    pub captured: bool,
    /// How far the mtime anchor would have been from the captured one: the write lag.
    pub mtime_lag: f64,
}

/// ffmpeg's segment list ("name,start,end" lines, starts on the capture clock) mapped to wall
/// time: name -> when its first frame was captured. With B-frames these starts sit 3 frames past
/// the demuxer's pts; subtracting that looks right and measured wrong (+10 ms as is, +40 ms
/// corrected, A/B at 120 fps).
pub fn parse_segment_list(csv: &str, clock_zero: f64) -> HashMap<String, f64> {
    let parsed: Option<HashMap<String, f64>> = csv
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut fields = l.split(',');
            let name = fields.next()?.trim().to_owned();
            let start: f64 = fields.next()?.trim().parse().ok()?;
            Some((name, clock_zero + start))
        })
        .collect();
    // A row cut mid-rewrite makes the whole list untrusted: mtimes then.
    parsed.unwrap_or_default()
}

/// `segments` in any order; `listed` from parse_segment_list (empty without a capture clock);
/// `capture_age` is how long ago the capture (re)started.
pub fn select(
    segments: &[Segment],
    listed: &HashMap<String, f64>,
    save_time: f64,
    clip_seconds: u32,
    capture_age: f64,
) -> Result<Selection, GiveUp> {
    if segments.is_empty() {
        // Judged on elapsed time, never on whether ffmpeg is alive: a health restart makes
        // that flicker.
        return Err(if capture_age < TOO_SOON_S { GiveUp::TooSoon } else { GiveUp::Error });
    }
    let mut sorted = segments.to_vec();
    sorted.sort_by(|a, b| a.mtime.total_cmp(&b.mtime));
    sorted.pop(); // the one ffmpeg is still writing
    if sorted.is_empty() {
        return Err(GiveUp::TooSoon);
    }
    let complete = sorted;
    let seg = SEGMENT_DURATION as f64;
    // A segment opens when the previous one closes; the first has no previous one.
    let mtime_open = |i: usize| if i > 0 { complete[i - 1].mtime } else { complete[0].mtime - seg };
    let opened = |i: usize| match listed.get(&complete[i].name) {
        // A listed start over two segments before the file's mtime is a stale row.
        Some(&t) if t > complete[i].mtime - 2.0 * seg => t,
        _ => mtime_open(i),
    };
    let n = complete.len();
    let oldest = (0..n).rev().find(|&i| save_time - opened(i) >= clip_seconds as f64).unwrap_or(0);
    let window_start = opened(oldest);
    let mtime_lag = mtime_open(oldest) - window_start;
    Ok(Selection {
        segments: complete[oldest..].to_vec(),
        duration: save_time - window_start,
        window_start,
        captured: mtime_lag != 0.0,
        mtime_lag,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAVE: f64 = 10_000.0;

    /// `count` segments a second apart, the newest written `newest_age` before the save.
    fn segments(count: usize, newest_age: f64) -> Vec<Segment> {
        (0..count)
            .map(|i| Segment { name: format!("seg_{i:03}.ts"), mtime: SAVE - newest_age - (count - 1 - i) as f64 })
            .collect()
    }

    fn names(s: &Selection) -> Vec<&str> {
        s.segments.iter().map(|x| x.name.as_str()).collect()
    }

    #[test]
    fn the_segment_being_written_is_never_selected() {
        let segs = segments(40, 0.3);
        let s = select(&segs, &HashMap::new(), SAVE, 5, 60.0).unwrap();
        assert!(!names(&s).contains(&"seg_039.ts"), "{:?}", names(&s));
        assert_eq!(names(&s).last(), Some(&"seg_038.ts"));
    }

    #[test]
    fn enough_segments_to_cover_the_clip_length() {
        let s = select(&segments(40, 0.3), &HashMap::new(), SAVE, 15, 60.0).unwrap();
        assert!(s.duration >= 15.0 && s.duration < 15.0 + 2.0 * SEGMENT_DURATION as f64, "{}", s.duration);
        assert!((s.duration - (SAVE - s.window_start)).abs() < 1e-9, "the audio window and the video -t differ");
    }

    #[test]
    fn the_window_opens_where_the_oldest_segment_opened_not_where_it_closed() {
        // Normal path: the segment before the selection closed when the oldest one opened.
        let segs = segments(40, 0.3);
        let s = select(&segs, &HashMap::new(), SAVE, 5, 60.0).unwrap();
        let oldest = segs.iter().position(|x| x.name == s.segments[0].name).unwrap();
        assert!((s.window_start - segs[oldest - 1].mtime).abs() < 1e-9);
        // Not enough history: no earlier segment, so its own mtime minus one segment (using the
        // mtime itself put the audio a whole segment late).
        let few = segments(4, 0.3);
        let s = select(&few, &HashMap::new(), SAVE, 30, 60.0).unwrap();
        assert_eq!(s.segments.len(), 3);
        assert!((s.window_start - (few[0].mtime - SEGMENT_DURATION as f64)).abs() < 0.05, "{}", s.window_start);
        assert!(!s.captured);
    }

    #[test]
    fn the_clip_is_anchored_on_capture_time_not_write_time() {
        let segs = segments(40, 0.3);
        let lag = 0.130;
        let csv: String = segs[..39]
            .iter()
            .map(|x| format!("{},{:.6},{:.6}\n", x.name, x.mtime - lag - 1.0 - 1000.0, x.mtime - lag - 1000.0))
            .collect();
        let listed = parse_segment_list(&csv, 1000.0);
        let s = select(&segs, &listed, SAVE, 5, 60.0).unwrap();
        let oldest = &s.segments[0];
        assert!((s.window_start - (oldest.mtime - lag - 1.0)).abs() < 0.001, "opens {} off", s.window_start - (oldest.mtime - lag - 1.0));
        assert!(s.captured && (s.mtime_lag - lag).abs() < 0.001, "lag {}", s.mtime_lag);
    }

    #[test]
    fn a_stale_list_row_never_anchors_the_clip() {
        // A failed rename leaves rows for names since reused, a whole wrap earlier.
        let segs = segments(8, 0.3);
        let listed: HashMap<String, f64> = segs.iter().map(|x| (x.name.clone(), x.mtime - 9.0)).collect();
        let s = select(&segs, &listed, SAVE, 5, 60.0).unwrap();
        assert!(s.segments.len() > 1 && s.duration < 7.0, "{} segments, {} s", s.segments.len(), s.duration);
        assert!(!s.captured);
    }

    #[test]
    fn giving_up_says_why() {
        assert_eq!(select(&[], &HashMap::new(), SAVE, 30, 1.0), Err(GiveUp::TooSoon));
        assert_eq!(select(&[], &HashMap::new(), SAVE, 30, 60.0), Err(GiveUp::Error), "a capture that wrote nothing in a minute is broken");
        assert_eq!(select(&segments(1, 0.3), &HashMap::new(), SAVE, 30, 60.0), Err(GiveUp::TooSoon));
    }

    #[test]
    fn segments_are_ordered_by_mtime_whatever_their_names() {
        // segment_wrap reuses names: seg_000 can be the newest.
        let mut segs = segments(10, 0.3);
        segs.rotate_left(3);
        let s = select(&segs, &HashMap::new(), SAVE, 3, 60.0).unwrap();
        let mtimes: Vec<f64> = s.segments.iter().map(|x| x.mtime).collect();
        assert!(mtimes.windows(2).all(|w| w[0] < w[1]));
        assert!(!names(&s).contains(&"seg_009.ts"));
    }

    #[test]
    fn a_broken_list_is_just_empty() {
        assert!(parse_segment_list("garbage\nseg_001.ts,notanumber,2\n", 0.0).is_empty());
        assert_eq!(parse_segment_list("seg_001.ts,1.5,2.5\n", 100.0).get("seg_001.ts"), Some(&101.5));
    }
}
