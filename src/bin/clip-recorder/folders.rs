//! Known folders through the API, never ~\Videos: a relocated or OneDrive-backed Videos library
//! lives elsewhere, and the on-disk name does not follow the display language.

use std::path::PathBuf;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_Videos, SHGetKnownFolderPath};

pub fn videos() -> PathBuf {
    unsafe {
        let mut ptr = std::ptr::null_mut();
        if SHGetKnownFolderPath(&FOLDERID_Videos, 0, std::ptr::null_mut(), &mut ptr) == 0 {
            let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
            let path = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            CoTaskMemFree(ptr as _);
            return PathBuf::from(path);
        }
        CoTaskMemFree(ptr as _);
    }
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Videos")
}

/// Where clips go: the configured folder, else Videos\ClipRecorder.
pub fn clips(configured: &str) -> PathBuf {
    if configured.trim().is_empty() { videos().join("ClipRecorder") } else { PathBuf::from(configured) }
}
