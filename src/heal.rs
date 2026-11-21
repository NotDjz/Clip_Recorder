//! When to reopen an audio stream, on each health tick (every 2 s). Pure, one per stream.
//!
//! A loopback stream can die hours in (sleep/resume, a driver reset): 5 saves in 70 lost the
//! game audio that way in one real log. A dead stream is reopened, with a wait that doubles on
//! every reopen, so one that opens and dies at once backs off too, and after MAX_REOPENS it is
//! left alone (a gone device must not flood the capped log). Only a stream that delivers resets
//! it. An open stream with no frame yet is never reopened: that is just silence. One that has
//! delivered and then nothing for STALL_S is stalled (seen: 3.7 h "alive" with no data), and is
//! reopened like a dead one.

pub const MAX_REOPENS: u32 = 6;
/// Seconds without a chunk, after the stream has delivered, that count as a stall.
pub const STALL_S: f64 = 10.0;
/// Ticks between two looks for a device when a side has none (30 s): one plugged in after
/// launch is picked up, and looking costs only an enumeration.
pub const DETECT_TICKS: u32 = 15;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    Keep,
    Reopen,
    /// Once, when the reopens run out: log it and leave the stream alone.
    GiveUp,
    /// No device: look for one again.
    Detect,
}

/// One stream's state. Never shared: a gone mic once held a dead loopback back.
#[derive(Debug, Default, Clone)]
pub struct Retry {
    wait: u32,
    reopens: u32,
    /// Ticks spent without a device since the last look.
    searching: u32,
}

impl Retry {
    /// `frames` delivered since the stream opened; `idle_s` since its newest chunk; `continuous`:
    /// it had delivered all along until then (delivered_continuously), so a gap is not silence.
    pub fn decide(&mut self, has_device: bool, alive: bool, frames: u64, idle_s: f64, continuous: bool) -> Decision {
        if !has_device {
            self.searching += 1;
            if self.searching < DETECT_TICKS {
                return Decision::Keep;
            }
            self.searching = 0;
            return Decision::Detect;
        }
        // A loopback that skips silence goes quiet with the game: only a continuous one stalls.
        if alive && frames > 0 && (idle_s < STALL_S || !continuous) {
            *self = Retry::default();
            return Decision::Keep;
        }
        if alive && frames == 0 {
            return Decision::Keep; // open, nothing delivered yet: silence
        }
        self.backoff()
    }

    /// A dead or stalled stream: reopen it unless still waiting, or once out of reopens.
    fn backoff(&mut self) -> Decision {
        if self.wait > 0 {
            self.wait -= 1;
            return Decision::Keep;
        }
        if self.reopens >= MAX_REOPENS {
            if self.reopens == MAX_REOPENS {
                self.reopens += 1;
                return Decision::GiveUp;
            }
            return Decision::Keep;
        }
        self.reopens += 1;
        self.wait = (1 << self.reopens) - 1;
        Decision::Reopen
    }
}

/// The device a dead stream reopens on. A pinned device that is missing stays the one waited
/// for: the default it fell back to keeps playing meanwhile, and a re-plugged headset is picked
/// up again; switching to the default for good lost it until a restart. Auto follows the
/// default. With nothing found, the device it had is retried.
pub fn reopen_on<D>(pinned: bool, had: Option<D>, configured: Option<D>, default: Option<D>) -> Option<D> {
    if pinned { configured.or(had) } else { default.or(had) }
}

/// Whether a stream delivered at least 90 % of the samples its rate owed between its open and its
/// newest chunk. This PC's loopback delivers even in silence; others skip it, and for those a
/// quiet game is no stall.
pub fn delivered_continuously(frames: u64, rate: u32, opened: f64, newest: f64) -> bool {
    frames as f64 >= 0.9 * rate as f64 * (newest - opened)
}

/// Whether a side keeps its history across a reopen or a device change: only in the same
/// format, since render_window cannot mix two.
pub fn keeps_history(before: (u32, u16), after: (u32, u16)) -> bool {
    before == after
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Silence after a reopen is not delivery: the backoff carries on if the stream dies again,
    /// instead of starting over and reopening every tick.
    #[test]
    fn silence_after_a_reopen_does_not_reset_the_backoff() {
        let mut r = Retry::default();
        assert_eq!(r.decide(true, false, 0, 0.0, true), Decision::Reopen);
        assert_eq!(r.decide(true, true, 0, 0.0, true), Decision::Keep, "open and silent");
        assert_eq!(
            r.decide(true, false, 0, 0.0, true),
            Decision::Keep,
            "dead again: still waiting"
        );
    }

    #[test]
    fn a_pinned_device_unplugged_is_waited_for_not_replaced() {
        // Unplugged: the default it fell back to stays; replugged: back on the headset.
        assert_eq!(
            reopen_on(true, Some("speakers"), None, Some("speakers")),
            Some("speakers")
        );
        assert_eq!(
            reopen_on(true, Some("speakers"), Some("headset"), Some("speakers")),
            Some("headset")
        );
        assert_eq!(
            reopen_on(true, Some("headset"), None, None),
            Some("headset"),
            "nothing found: retry it"
        );
    }

    #[test]
    fn auto_follows_the_default_device() {
        assert_eq!(
            reopen_on(false, Some("old"), None, Some("new default")),
            Some("new default")
        );
        assert_eq!(reopen_on(false, Some("old"), None, None), Some("old"));
    }

    #[test]
    fn history_survives_only_an_unchanged_format() {
        assert!(keeps_history((48000, 2), (48000, 2)));
        assert!(!keeps_history((48000, 2), (44100, 2)));
        assert!(!keeps_history((48000, 2), (48000, 6)));
    }

    #[test]
    fn a_stream_that_dies_late_is_reopened_at_once() {
        let mut r = Retry::default();
        for _ in 0..20 {
            assert_eq!(r.decide(true, true, 1000, 0.0, true), Decision::Keep);
        }
        assert_eq!(
            r.decide(true, false, 1000, 0.0, true),
            Decision::Reopen,
            "a late death waited"
        );
    }

    #[test]
    fn an_open_stream_with_no_frame_yet_is_left_alone() {
        let mut r = Retry::default();
        for _ in 0..100 {
            assert_eq!(
                r.decide(true, true, 0, 0.0, true),
                Decision::Keep,
                "silence was taken for a dead stream"
            );
        }
    }

    #[test]
    fn a_side_without_a_device_looks_again_every_30_s() {
        // A headset plugged in after launch (Start with Windows) was never picked up.
        let mut r = Retry::default();
        let looked: Vec<u32> = (1..=45)
            .filter(|_| r.decide(false, false, 0, f64::INFINITY, true) == Decision::Detect)
            .collect();
        assert_eq!(looked, [DETECT_TICKS, 2 * DETECT_TICKS, 3 * DETECT_TICKS]);
        assert_eq!(DETECT_TICKS, 15, "every 30 s at one tick every 2 s");
        // Found: the stream opens, delivers, and the side is healed like any other.
        assert_eq!(r.decide(true, true, 100, 0.0, true), Decision::Keep);
        assert_eq!(r.decide(true, false, 100, 0.0, true), Decision::Reopen);
    }

    #[test]
    fn a_device_that_stays_gone_backs_off_then_is_left_alone() {
        let mut r = Retry::default();
        let mut reopened = Vec::new();
        let mut gave_up = Vec::new();
        for tick in 1..=300 {
            match r.decide(true, false, 0, 0.0, true) {
                Decision::Reopen => reopened.push(tick),
                Decision::GiveUp => gave_up.push(tick),
                Decision::Keep | Decision::Detect => {}
            }
        }
        let gaps: Vec<u32> = reopened.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(gaps, [2, 4, 8, 16, 32], "reopened at ticks {reopened:?}");
        assert_eq!(gave_up.len(), 1, "the give-up is logged once, not {gave_up:?}");
    }

    #[test]
    fn a_stream_that_delivers_resets_the_backoff() {
        let mut r = Retry::default();
        assert_eq!(r.decide(true, false, 0, 0.0, true), Decision::Reopen);
        assert_eq!(r.decide(true, true, 500, 0.0, true), Decision::Keep); // the reopen worked
        assert_eq!(
            r.decide(true, false, 500, 0.0, true),
            Decision::Reopen,
            "the next death waited for an old backoff"
        );
    }

    /// 2026-11-14: a loopback delivered 10.6 s after a reopen, then nothing for 3.7 h while
    /// still "alive", and two clips lost the game sound. A stream that has delivered and then
    /// goes quiet for STALL_S is reopened like a dead one.
    #[test]
    fn a_loopback_that_stops_delivering_is_reopened() {
        let mut r = Retry::default();
        assert_eq!(r.decide(true, true, 507_360, 0.0, true), Decision::Keep);
        assert_eq!(
            r.decide(true, true, 507_360, 13_258.6, true),
            Decision::Reopen,
            "stalled for hours"
        );
    }

    #[test]
    fn a_short_quiet_moment_is_not_a_stall() {
        let mut r = Retry::default();
        assert_eq!(r.decide(true, true, 1000, STALL_S - 0.1, true), Decision::Keep);
        assert_eq!(r.decide(true, true, 1000, STALL_S, true), Decision::Reopen);
        // Just reopened: nothing delivered yet, whatever the old history says, is silence.
        assert_eq!(r.decide(true, true, 0, 13_000.0, true), Decision::Keep);
    }

    /// A loopback that skips silence (other hardware than this PC's) is not stalled when the game
    /// goes quiet: only one that delivered all along can stall, or every pause would reopen it.
    #[test]
    fn a_loopback_that_skips_silence_is_not_reopened_when_quiet() {
        let mut r = Retry::default();
        assert_eq!(r.decide(true, true, 48_000, 60.0, false), Decision::Keep);
        assert_eq!(r.decide(true, true, 48_000, 60.0, true), Decision::Reopen);
    }

    #[test]
    fn continuous_means_nearly_every_sample_since_the_open() {
        // 2026-11-14: 507 360 frames at 48 kHz over the 10.6 s it delivered after its reopen.
        assert!(delivered_continuously(507_360, 48_000, 1000.0, 1010.6));
        assert!(
            !delivered_continuously(48_000, 48_000, 1000.0, 1010.0),
            "1 s over 10 s skips silence"
        );
        assert!(
            delivered_continuously(43_200, 48_000, 1000.0, 1001.0),
            "90 % is continuous"
        );
        assert!(!delivered_continuously(43_000, 48_000, 1000.0, 1001.0));
    }
}
