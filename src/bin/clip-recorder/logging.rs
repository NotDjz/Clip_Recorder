//! log(): one timestamped line in %APPDATA%\ClipRecorder\clip_recorder.log. Never fails the
//! caller: a log that cannot be written must not take a save down with it.

use clip_recorder::{log, paths};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOCK: Mutex<()> = Mutex::new(());

pub fn log(msg: &str) {
    // Unit tests run without CLIPREC_DATA_DIR: they must never write into a real user's log.
    if cfg!(test) {
        return;
    }
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let line = format!("[{}] {msg}", log::rfc3339(ms));
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = paths::log_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = log::append_capped(&path, &line, log::LOG_MAX_BYTES);
}
