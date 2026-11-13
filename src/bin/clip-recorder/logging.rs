//! log(): one timestamped line in %APPDATA%\ClipRecorder\clip_recorder.log. Never fails the
//! caller: a log that cannot be written must not take a save down with it.

use clip_recorder::{log, paths};
use std::sync::Mutex;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

static LOCK: Mutex<()> = Mutex::new(());

pub fn log(msg: &str) {
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    let line = format!(
        "[{:04}-{:02}-{:02} {:02}:{:02}:{:02}] {msg}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    );
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = paths::log_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = log::append_capped(&path, &line, log::LOG_MAX_BYTES);
}
