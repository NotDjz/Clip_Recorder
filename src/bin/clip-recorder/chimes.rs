//! Plays the synthesised chimes. PlaySoundW refuses SND_ASYNC with SND_MEMORY, so each play
//! is synchronous on a thread of its own: on the calling thread it would stall the UI (the press
//! chime fires on it) and hold the banner back for the sound's length.

use crate::logging::log;
use clip_recorder::tone;
use std::sync::OnceLock;
use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_MEMORY, SND_NODEFAULT};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Chime {
    Start,
    Saved,
    Failed,
}

pub fn play(chime: Chime) {
    static WAVS: OnceLock<[Vec<u8>; 3]> = OnceLock::new();
    let wavs = WAVS.get_or_init(|| [tone::wav(&tone::START), tone::wav(&tone::SAVE), tone::wav(&tone::FAIL)]);
    let wav: &'static [u8] = match chime {
        Chime::Start => &wavs[0],
        Chime::Saved => &wavs[1],
        Chime::Failed => &wavs[2],
    };
    std::thread::spawn(move || {
        if unsafe { PlaySoundW(wav.as_ptr() as *const u16, std::ptr::null_mut(), SND_MEMORY | SND_NODEFAULT) } == 0 {
            log("chime not played");
        }
    });
}
