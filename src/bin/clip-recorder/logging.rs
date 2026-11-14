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
        panic_guard()
    } else {
        Some(LOCK.lock().unwrap_or_else(|e| e.into_inner()))
    };
    let path = paths::log_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = log::append_capped(&path, &line, log::LOG_MAX_BYTES);
}

/// The log lock for a panic report. Another thread writing holds it for well under a second, so
/// wait that long: written alongside, the report could be lost to its rotation. Past that, this
/// thread most likely holds it itself (the panic struck inside a write): go on unlocked rather
/// than deadlock.
fn panic_guard() -> Option<std::sync::MutexGuard<'static, ()>> {
    use std::sync::TryLockError;
    for _ in 0..100 {
        match LOCK.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(e)) => return Some(e.into_inner()),
            Err(TryLockError::WouldBlock) => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
    None
}
