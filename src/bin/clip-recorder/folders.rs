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

/// The folder picker. ponytail: the classic SHBrowseForFolder tree; IFileOpenDialog with
/// FOS_PICKFOLDERS looks newer but needs COM vtables windows-sys does not have.
pub fn pick(owner: windows_sys::Win32::Foundation::HWND, title: &str) -> Option<String> {
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::UI::Shell::{BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, SHBrowseForFolderW, SHGetPathFromIDListW};
    let title = crate::winutil::wide(title);
    unsafe {
        // The new-style dialog needs an STA; already initialised is fine.
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let info = BROWSEINFOW {
            hwndOwner: owner,
            lpszTitle: title.as_ptr(),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            ..std::mem::zeroed()
        };
        let list = SHBrowseForFolderW(&info);
        if list.is_null() {
            return None;
        }
        let mut path = [0u16; 32768];
        let ok = SHGetPathFromIDListW(list, path.as_mut_ptr()) != 0;
        CoTaskMemFree(list as _);
        let len = path.iter().position(|&c| c == 0).unwrap_or(0);
        (ok && len > 0).then(|| String::from_utf16_lossy(&path[..len]))
    }
}
