//! log(): one timestamped line in %APPDATA%\ClipRecorder\clip_recorder.log. Never fails the
//! caller: a log that cannot be written must not take a save down with it.

use clip_recorder::{log, paths};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOCK: Mutex<()> = Mutex::new(());

pub fn log(msg: &str) {
    write(msg, false);
}

/// Logs every panic, in any thread, before the default handling. First thing in main().
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let location = info
            .location()
            .map_or(String::new(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let report = log::panic_report(
            thread.name().unwrap_or("unnamed"),
            info.payload_as_str().unwrap_or("(no message)"),
            &location,
            &backtrace,
        );
        // Never wait on the log lock here: the panic may have struck while this thread held it.
        write(&report, true);
        previous(info);
    }));
}

fn write(msg: &str, from_panic: bool) {
    // Unit tests run without CLIPREC_DATA_DIR: they must never write into a real user's log.
    if cfg!(test) {
        return;
    }
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let line = format!("[{}] {msg}", log::rfc3339(ms));
    let _guard = if from_panic {
        LOCK.try_lock().ok()
    } else {
        Some(LOCK.lock().unwrap_or_else(|e| e.into_inner()))
    };
    let path = paths::log_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = log::append_capped(&path, &line, log::LOG_MAX_BYTES);
}
