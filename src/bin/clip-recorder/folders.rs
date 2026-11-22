//! Known folders through the API, never ~\Videos: a relocated or OneDrive-backed Videos library
//! lives elsewhere, and the on-disk name does not follow the display language.

use std::path::{Path, PathBuf};
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
    if configured.trim().is_empty() {
        videos().join("ClipRecorder")
    } else {
        PathBuf::from(configured.trim())
    }
}

/// The folder picker, opened at `start`. Never the classic SHBrowseForFolder tree: it built the
/// whole shell namespace before showing, and when that was slow the user had moved on, so it
/// opened behind their other window with Settings disabled under it (the app looked frozen).
pub fn pick(owner: windows_sys::Win32::Foundation::HWND, title: &str, start: &Path) -> Option<String> {
    use windows::Win32::Foundation::ERROR_CANCELLED;
    match unsafe { show_picker(owner, title, start) } {
        Ok(path) => Some(path),
        Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => None,
        Err(e) => {
            crate::logging::log(&format!("folder picker failed: {e}"));
            None
        }
    }
}

unsafe fn show_picker(
    owner: windows_sys::Win32::Foundation::HWND,
    title: &str,
    start: &Path,
) -> windows::core::Result<String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::Win32::UI::Shell::{
        FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
        SIGDN_FILESYSPATH,
    };
    use windows::core::HSTRING;
    unsafe {
        // The dialog needs an STA; already initialised is fine.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
        dialog.SetTitle(&HSTRING::from(title))?;
        if let Some(folder) = start_at(start)
            && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(folder.as_path()), None)
        {
            dialog.SetFolder(&item)?;
        }
        dialog.Show(Some(HWND(owner)))?; // Cancel is ERROR_CANCELLED
        let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = String::from_utf16_lossy(name.as_wide());
        CoTaskMemFree(name.0 as _);
        Ok(path)
    }
}

/// Where the picker opens: the nearest folder that exists (Videos\ClipRecorder is only created by
/// the first save), never a network one: an offline share stalls the UI thread before it shows.
fn start_at(folder: &Path) -> Option<PathBuf> {
    if on_network(folder) {
        return None;
    }
    folder.ancestors().find(|p| p.is_dir()).map(Path::to_path_buf)
}

/// A UNC path or a mapped network drive, told from the path and the drive's type: neither
/// touches the network.
fn on_network(folder: &Path) -> bool {
    use std::path::{Component, Prefix};
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    const DRIVE_REMOTE: u32 = 4; // winbase.h (windows-sys files it under WindowsProgramming)
    let Some(Component::Prefix(prefix)) = folder.components().next() else {
        return false;
    };
    match prefix.kind() {
        Prefix::UNC(..) | Prefix::VerbatimUNC(..) => true,
        Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
            let root = crate::winutil::wide(&format!("{}:\\", d as char));
            unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_folder_is_used_without_its_spaces() {
        assert_eq!(clips("  D:/Clips \t"), PathBuf::from("D:/Clips"));
    }

    #[test]
    fn the_picker_starts_at_the_nearest_folder_that_exists() {
        let dir = std::env::temp_dir();
        assert_eq!(start_at(&dir.join("not yet").join("created")), Some(dir.clone()));
        assert_eq!(start_at(&dir), Some(dir));
    }

    #[test]
    fn the_picker_never_starts_on_a_network_folder() {
        // An offline share would stall the UI thread before the dialog shows.
        assert!(on_network(Path::new(r"\\no-such-server\share\clips")));
        assert!(on_network(Path::new(r"\\?\UNC\no-such-server\share")));
        assert!(!on_network(&std::env::temp_dir()));
        assert_eq!(start_at(Path::new(r"\\no-such-server\share\clips")), None);
    }
}
