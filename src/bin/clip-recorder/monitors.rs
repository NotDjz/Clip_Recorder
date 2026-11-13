//! The screens, primary first: the order Settings shows and ddagrab's output index follows.

use clip_recorder::ffmpeg_cmd::Monitor;
use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, MONITORINFOF_PRIMARY, SM_CXSCREEN, SM_CYSCREEN};

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub name: String,
    pub rect: Monitor,
    pub primary: bool,
}

unsafe extern "system" fn visit(handle: HMONITOR, _dc: HDC, _rect: *mut RECT, data: LPARAM) -> i32 {
    let screens = unsafe { &mut *(data as *mut Vec<Screen>) };
    let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if unsafe { GetMonitorInfoW(handle, &mut info as *mut MONITORINFOEXW as *mut _) } != 0 {
        let r = info.monitorInfo.rcMonitor;
        let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
        screens.push(Screen {
            name: String::from_utf16_lossy(&info.szDevice[..len]),
            rect: Monitor { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top },
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }
    1
}

pub fn list() -> Vec<Screen> {
    let mut screens: Vec<Screen> = Vec::new();
    unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(visit), &mut screens as *mut _ as LPARAM) };
    screens.sort_by_key(|s| (!s.primary, s.rect.x, s.rect.y));
    if screens.is_empty() {
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        screens.push(Screen { name: "Default".into(), rect: Monitor { x: 0, y: 0, w, h }, primary: true });
    }
    screens
}
