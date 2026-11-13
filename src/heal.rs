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

#[cfg(test)]
mod tests {
    use super::*;

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
            assert_eq!(r.decide(true, true, 0), Decision::Keep, "silence was taken for a dead stream");
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
        assert_eq!(r.decide(true, false, 500), Decision::Reopen, "the next death waited for an old backoff");
    }
}
