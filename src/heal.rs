//! When to reopen an audio stream, on each health tick (every 2 s). Pure, one per stream.
//!
//! A loopback stream can die hours in (sleep/resume, a driver reset): 5 saves in 70 lost the
//! game audio that way in one real log. A dead stream is reopened, with a wait that doubles on
//! every reopen, so one that opens and dies at once backs off too, and after MAX_REOPENS it is
//! left alone (a gone device must not flood the capped log). Only a stream that delivers resets
//! it. An open stream with no frame yet is never reopened: that is just silence.

pub const MAX_REOPENS: u32 = 6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    Keep,
    Reopen,
    /// Once, when the reopens run out: log it and leave the stream alone.
    GiveUp,
}

/// One stream's state. Never shared: a gone mic once held a dead loopback back.
#[derive(Debug, Default, Clone)]
pub struct Retry {
    wait: u32,
    reopens: u32,
}

impl Retry {
    pub fn decide(&mut self, has_device: bool, alive: bool, frames: u64) -> Decision {
        if !has_device || (alive && frames > 0) {
            *self = Retry::default();
            return Decision::Keep;
        }
        if alive {
            return Decision::Keep; // open, nothing delivered yet: silence
        }
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
        assert_eq!(r.decide(true, false, 0), Decision::Reopen);
        assert_eq!(r.decide(true, true, 0), Decision::Keep, "open and silent");
        assert_eq!(r.decide(true, false, 0), Decision::Keep, "dead again: still waiting");
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
            assert_eq!(r.decide(true, true, 1000), Decision::Keep);
        }
        assert_eq!(r.decide(true, false, 1000), Decision::Reopen, "a late death waited");
    }

    #[test]
    fn an_open_stream_with_no_frame_yet_is_left_alone() {
        let mut r = Retry::default();
        for _ in 0..100 {
            assert_eq!(
                r.decide(true, true, 0),
                Decision::Keep,
                "silence was taken for a dead stream"
            );
        }
    }

    #[test]
    fn no_device_means_nothing_to_heal() {
        assert_eq!(Retry::default().decide(false, false, 0), Decision::Keep);
    }

    #[test]
    fn a_device_that_stays_gone_backs_off_then_is_left_alone() {
        let mut r = Retry::default();
        let mut reopened = Vec::new();
        let mut gave_up = Vec::new();
        for tick in 1..=300 {
            match r.decide(true, false, 0) {
                Decision::Reopen => reopened.push(tick),
                Decision::GiveUp => gave_up.push(tick),
                Decision::Keep => {}
            }
        }
        let gaps: Vec<u32> = reopened.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(gaps, [2, 4, 8, 16, 32], "reopened at ticks {reopened:?}");
        assert_eq!(gave_up.len(), 1, "the give-up is logged once, not {gave_up:?}");
    }

    #[test]
    fn a_stream_that_delivers_resets_the_backoff() {
        let mut r = Retry::default();
        assert_eq!(r.decide(true, false, 0), Decision::Reopen);
        assert_eq!(r.decide(true, true, 500), Decision::Keep); // the reopen worked
        assert_eq!(
            r.decide(true, false, 500),
            Decision::Reopen,
            "the next death waited for an old backoff"
        );
    }
}
