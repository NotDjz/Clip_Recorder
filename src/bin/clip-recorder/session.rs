//! Restart Manager (MSI upgrades and uninstalls) and logoff close apps through their top-level
//! windows. A tray app shows none, so it keeps a hidden one: without it, an upgrade with the
//! app running asks for a reboot. Not util:CloseApplication in the MSI: it sends WM_CLOSE to
//! winit's hidden windows too, leaving a windowless process that blocks the upgrade (papercut).

use crate::logging::log;
use crate::winutil::wide;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Recovery::{
    RESTART_NO_CRASH, RESTART_NO_HANG, RESTART_NO_REBOOT, RegisterApplicationRestart,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, WM_ENDSESSION, WNDCLASSW, WS_OVERLAPPED,
};

pub const CLASS: &str = "ClipRecorderSession";

unsafe extern "system" fn on_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_ENDSESSION && wparam != 0 {
        log("Windows is ending the session (upgrade, uninstall or logoff): exiting");
        // The kill-on-close job takes the capture ffmpeg with us.
        std::process::exit(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Must run on the thread that pumps messages (the UI thread). Also asks Restart Manager to
/// start the app again after an upgrade.
pub fn listen() {
    unsafe {
        let class_name = wide(CLASS);
        let instance = GetModuleHandleW(std::ptr::null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(on_message),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&class);
        // Top-level and never shown: a message-only window (HWND_MESSAGE) gets no session broadcast.
        let hwnd = CreateWindowExW(
            0, class_name.as_ptr(), class_name.as_ptr(), WS_OVERLAPPED,
            0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), instance, std::ptr::null(),
        );
        if hwnd.is_null() {
            log("session window not created: an upgrade with the app running will ask for a reboot");
        }
        RegisterApplicationRestart(std::ptr::null(), RESTART_NO_CRASH | RESTART_NO_HANG | RESTART_NO_REBOOT);
    }
}
