//! Clip Recorder's pure logic: everything that decides something and can be tested without a
//! device, a window or a running ffmpeg. The Win32 and UI side lives in src/bin/clip-recorder.

pub mod config;
pub mod log;
pub mod paths;
