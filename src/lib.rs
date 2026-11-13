//! Clip Recorder's pure logic: everything that decides something and can be tested without a
//! device, a window or a running ffmpeg. The Win32 and UI side lives in src/bin/clip-recorder.

pub mod clock;
pub mod config;
pub mod ffmpeg_cmd;
pub mod heal;
pub mod hotkey;
pub mod log;
pub mod paths;
pub mod render;
pub mod selection;
pub mod settings;
pub mod tone;
