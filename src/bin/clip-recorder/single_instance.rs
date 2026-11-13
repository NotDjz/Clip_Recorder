//! One Clip Recorder per session: two would fight over the capture, the hotkey and the GPU's
//! few NVENC sessions. Same mutex name as the Python app, so the two versions never run together.

use crate::winutil::wide;
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::Threading::CreateMutexW;

/// True for the first instance. The mutex handle is leaked on purpose: it lives as long as the
/// process. A test run with its own data folder gets its own mutex.
pub fn acquire() -> bool {
    let mut name = String::from("ClipRecorder_SingleInstance");
    if let Some(dir) = std::env::var_os("CLIPREC_DATA_DIR").filter(|d| !d.is_empty()) {
        // A '\' after the name would read as a namespace separator.
        name.push('.');
        name.push_str(&dir.to_string_lossy().replace('\\', "/").to_lowercase());
    }
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, wide(&name).as_ptr());
        let error = GetLastError();
        // Access denied: an elevated instance holds it.
        !(error == ERROR_ALREADY_EXISTS || handle.is_null() && error == ERROR_ACCESS_DENIED)
    }
}
