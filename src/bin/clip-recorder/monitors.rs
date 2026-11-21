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
        let len = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
        screens.push(Screen {
            name: String::from_utf16_lossy(&info.szDevice[..len]),
            rect: Monitor {
                x: r.left,
                y: r.top,
                w: r.right - r.left,
                h: r.bottom - r.top,
            },
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }
    1
}

/// The NVIDIA adapter's outputs, in DXGI's order: what ddagrab's output_idx counts, since the
/// capture names that adapter to ffmpeg (ffmpeg_cmd::ddagrab_input) by the same rule, the first
/// adapter with NVIDIA's vendor id. Empty without one, or if DXGI cannot be asked: GDI then.
pub fn dxgi_outputs() -> Vec<Monitor> {
    use clip_recorder::ffmpeg_cmd::NVIDIA_VENDOR_ID;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    let outputs = || -> windows::core::Result<Vec<Monitor>> {
        let factory = unsafe { CreateDXGIFactory1::<IDXGIFactory1>()? };
        // EnumAdapters1 and EnumOutputs fail (DXGI_ERROR_NOT_FOUND) one past the last.
        let nvidia = (0..)
            .map_while(|i| unsafe { factory.EnumAdapters1(i) }.ok())
            .find(|a| unsafe { a.GetDesc1() }.is_ok_and(|d| d.VendorId == NVIDIA_VENDOR_ID));
        let Some(adapter) = nvidia else { return Ok(Vec::new()) };
        let mut found = Vec::new();
        while let Ok(output) = unsafe { adapter.EnumOutputs(found.len() as u32) } {
            let r = unsafe { output.GetDesc()? }.DesktopCoordinates;
            found.push(Monitor {
                x: r.left,
                y: r.top,
                w: r.right - r.left,
                h: r.bottom - r.top,
            });
        }
        Ok(found)
    };
    outputs().unwrap_or_else(|e| {
        crate::logging::log(&format!("DXGI outputs not listed ({e}): capturing through GDI"));
        Vec::new()
    })
}

pub fn list() -> Vec<Screen> {
    let mut screens: Vec<Screen> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(visit),
            &mut screens as *mut _ as LPARAM,
        )
    };
    screens.sort_by_key(|s| (!s.primary, s.rect.x, s.rect.y));
    if screens.is_empty() {
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        screens.push(Screen {
            name: "Default".into(),
            rect: Monitor { x: 0, y: 0, w, h },
            primary: true,
        });
    }
    screens
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs real screens: `cargo test -- --ignored --nocapture dxgi_outputs_on_this_machine`.
    /// Every screen of Settings' list is either a DXGI output of the first adapter or captured
    /// through GDI, and the mapping is printed to check by eye.
    #[test]
    #[ignore]
    fn dxgi_outputs_on_this_machine() {
        let outputs = dxgi_outputs();
        println!("DXGI outputs (NVIDIA adapter): {outputs:?}");
        for (i, s) in list().iter().enumerate() {
            let source =
                clip_recorder::ffmpeg_cmd::source(clip_recorder::ffmpeg_cmd::Grabber::Ddagrab, &outputs, s.rect);
            println!(
                "Settings {i}: {} {:?} primary={} -> {source:?}",
                s.name, s.rect, s.primary
            );
        }
        assert!(!outputs.is_empty(), "no DXGI output listed on a machine with a screen");
    }
}
