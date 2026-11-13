//! One Clip Recorder per session: two would fight over the capture, the hotkey and the GPU's
//! few NVENC sessions. Same mutex name as the Python app, so the two versions never run together.

use crate::winutil::wide;
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::Threading::CreateMutexW;

/// This instance's name: the mutex's, and the session window's title, by which a second launch
/// finds the first. A test run with its own data folder gets its own.
pub fn name() -> String {
    name_for(std::env::var_os("CLIPREC_DATA_DIR").as_deref())
}

/// Same name as the Python app's mutex without a data folder, so the two never run together.
fn name_for(data_dir: Option<&std::ffi::OsStr>) -> String {
    let mut name = String::from("ClipRecorder_SingleInstance");
    if let Some(dir) = data_dir.filter(|d| !d.is_empty()) {
        // A '\' after the name would read as a namespace separator.
        name.push('.');
        name.push_str(&dir.to_string_lossy().replace('\\', "/").to_lowercase());
    }
    name
}

/// True for the first instance. The mutex handle is leaked on purpose: it lives as long as the
/// process.
pub fn acquire() -> bool {
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, wide(&name()).as_ptr());
        let error = GetLastError();
        // Access denied: an elevated instance holds it.
        !(error == ERROR_ALREADY_EXISTS || handle.is_null() && error == ERROR_ACCESS_DENIED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installed_app_shares_the_python_mutex_and_a_test_run_gets_its_own() {
        assert_eq!(name_for(None), "ClipRecorder_SingleInstance");
        assert_eq!(name_for(Some("".as_ref())), "ClipRecorder_SingleInstance");
        let test = name_for(Some(r"C:\Temp\CrTest".as_ref()));
        assert_eq!(test, "ClipRecorder_SingleInstance.c:/temp/crtest");
        assert!(!test.contains('\\'), "a backslash would read as a namespace");
    }
}
