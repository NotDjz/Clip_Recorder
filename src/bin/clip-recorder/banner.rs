//! The "Saving clip / Clip saved / Clip not saved" pill in the corner of the captured screen.
//!
//! A native layered popup, styled before it is ever shown: click-through, never activated
//! (a windowed game would lose focus), and excluded from screen capture, or it would be burnt
//! into every clip. That is also why a screenshot of it comes out empty: the feature working.

use crate::winutil::wide;
use clip_recorder::ffmpeg_cmd::Monitor;
use std::cell::RefCell;
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State {
    /// The dot breathes, the length is dimmed: provisional.
    Saving,
    Saved,
    /// A hollow ring: a tally light that never lit.
    Failed,
}

const BG: u32 = 0x191C1B;
const FG: u32 = 0xE6EAE7;
const FG2: u32 = 0x8B958F;
const ACCENT: u32 = 0xFF4A3D;
const PROVISIONAL: u32 = 0x49524D;
/// The transparency key: never a colour of the design.
const KEY: u32 = 0xFF00FF;

pub const HOLD_MS: u32 = 3000;
pub const SAVING_MS: u32 = 3500;
const PULSE_MS: u32 = 70;
const PULSE_STEPS: u32 = 20;
const PULSE_FLOOR: f64 = 0.32;
// Layout at 96 DPI.
const MIN_W: i32 = 250;
const MIN_H: i32 = 40;
const PAD: i32 = 20;
const DOT_X: i32 = 23;
const DOT_R: i32 = 4;
const TEXT_X: i32 = 38;
const MARGIN: i32 = 24;

const TIMER_HIDE: usize = 1;
const TIMER_PULSE: usize = 2;
const CLASS: &str = "ClipRecorderBanner";

struct View {
    state: State,
    text: String,
    seconds: Option<u32>,
    step: u32,
}

thread_local! {
    static VIEW: RefCell<View> = RefCell::new(View { state: State::Saved, text: String::new(), seconds: None, step: 0 });
}

pub struct Banner {
    hwnd: HWND,
}

fn colorref(rgb: u32) -> COLORREF {
    ((rgb >> 16) & 0xFF) | (rgb & 0xFF00) | ((rgb & 0xFF) << 16)
}

/// The dot's colour while saving: ACCENT breathing down towards BG and back.
fn breath(step: u32) -> u32 {
    let level = PULSE_FLOOR + (1.0 - PULSE_FLOOR) * (0.5 + 0.5 * (std::f64::consts::TAU * step as f64 / PULSE_STEPS as f64).cos());
    let mix = |shift: u32| {
        let (a, b) = ((ACCENT >> shift) & 0xFF, (BG >> shift) & 0xFF);
        ((b as f64 + (a as f64 - b as f64) * level).round() as u32) << shift
    };
    mix(16) | mix(8) | mix(0)
}

impl Banner {
    /// Created hidden, with every style already applied.
    pub fn new() -> Banner {
        unsafe {
            let class_name = wide(CLASS);
            let instance = GetModuleHandleW(std::ptr::null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(on_message),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                ..std::mem::zeroed()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                class_name.as_ptr(), class_name.as_ptr(), WS_POPUP,
                0, 0, MIN_W, MIN_H, std::ptr::null_mut(), std::ptr::null_mut(), instance, std::ptr::null(),
            );
            SetLayeredWindowAttributes(hwnd, colorref(KEY), 0, LWA_COLORKEY);
            SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
            Banner { hwnd }
        }
    }

    /// Replaces whatever is showing. Hides itself after `duration_ms`.
    pub fn show(&self, state: State, text: &str, seconds: Option<u32>, duration_ms: u32, screen: Monitor) {
        VIEW.with(|v| *v.borrow_mut() = View { state, text: text.into(), seconds, step: 0 });
        unsafe {
            // A replaced banner's timers go with it: its pulse must not drive the new one's dot.
            KillTimer(self.hwnd, TIMER_HIDE);
            KillTimer(self.hwnd, TIMER_PULSE);
            let scale = GetDpiForWindow(self.hwnd).max(96) as i32;
            let (w, h) = measure(self.hwnd, text, seconds, scale);
            let px = |v: i32| v * scale / 96;
            SetWindowPos(
                self.hwnd, HWND_TOPMOST,
                screen.x + screen.w - w - px(MARGIN), screen.y + px(MARGIN), w, h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            InvalidateRect(self.hwnd, std::ptr::null(), 1);
            // Paint now: the save's synchronous half holds the thread next.
            UpdateWindow(self.hwnd);
            SetTimer(self.hwnd, TIMER_HIDE, duration_ms, None);
            if state == State::Saving {
                SetTimer(self.hwnd, TIMER_PULSE, PULSE_MS, None);
            }
        }
    }
}

unsafe fn font(dpi: i32, points: i32, bold: bool, face: &str) -> HFONT {
    let height = -(points * dpi / 72);
    unsafe {
        CreateFontW(height, 0, 0, 0, if bold { FW_BOLD as i32 } else { FW_NORMAL as i32 }, 0, 0, 0,
            DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32, CLEARTYPE_QUALITY as u32,
            DEFAULT_PITCH as u32, wide(face).as_ptr())
    }
}

fn seconds_text(seconds: Option<u32>) -> Option<String> {
    seconds.map(|s| format!("{s}s"))
}

/// Sized to its text: a fixed pill ran out at 125 % scaling.
unsafe fn measure(hwnd: HWND, text: &str, seconds: Option<u32>, dpi: i32) -> (i32, i32) {
    unsafe {
        let dc = GetDC(hwnd);
        let extent = |f: HFONT, s: &str| {
            let old = SelectObject(dc, f as _);
            let mut size = SIZE { cx: 0, cy: 0 };
            let w = wide(s);
            GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32 - 1, &mut size);
            SelectObject(dc, old);
            size
        };
        let status = font(dpi, 11, true, "Segoe UI");
        let mono = font(dpi, 10, false, "Consolas");
        let px = |v: i32| v * dpi / 96;
        let main = extent(status, text);
        let secs = seconds_text(seconds).map_or(0, |s| extent(mono, &s).cx + px(12));
        DeleteObject(status as _);
        DeleteObject(mono as _);
        ReleaseDC(hwnd, dc);
        ((px(TEXT_X) + main.cx + px(PAD) + secs).max(px(MIN_W)), (main.cy + px(18)).max(px(MIN_H)))
    }
}

unsafe fn paint(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(hwnd, &mut r);
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let px = |v: i32| v * dpi / 96;
        let key = CreateSolidBrush(colorref(KEY));
        FillRect(dc, &r, key);
        DeleteObject(key as _);
        // The pill: a round rect whose corners are as tall as it is.
        let bg = CreateSolidBrush(colorref(BG));
        let old_brush = SelectObject(dc, bg as _);
        let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        RoundRect(dc, 0, 0, r.right, r.bottom, r.bottom, r.bottom);
        VIEW.with(|v| {
            let v = v.borrow();
            let (cx, cy, rr) = (px(DOT_X), r.bottom / 2, px(DOT_R));
            if v.state == State::Failed {
                let ring = CreatePen(PS_SOLID, px(2), colorref(FG2));
                SelectObject(dc, ring as _);
                SelectObject(dc, GetStockObject(NULL_BRUSH));
                Ellipse(dc, cx - rr, cy - rr, cx + rr, cy + rr);
                SelectObject(dc, GetStockObject(NULL_PEN));
                DeleteObject(ring as _);
            } else {
                let colour = if v.state == State::Saving { breath(v.step) } else { ACCENT };
                let dot = CreateSolidBrush(colorref(colour));
                SelectObject(dc, dot as _);
                Ellipse(dc, cx - rr, cy - rr, cx + rr + 1, cy + rr + 1);
                SelectObject(dc, bg as _);
                DeleteObject(dot as _);
            }
            SetBkMode(dc, TRANSPARENT as i32);
            let status = font(dpi, 11, true, "Segoe UI");
            let old_font = SelectObject(dc, status as _);
            SetTextColor(dc, colorref(FG));
            let mut text_rect = RECT { left: px(TEXT_X), top: 0, right: r.right, bottom: r.bottom };
            let mut t = wide(&v.text);
            DrawTextW(dc, t.as_mut_ptr(), -1, &mut text_rect, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            if let Some(s) = seconds_text(v.seconds) {
                let mono = font(dpi, 10, false, "Consolas");
                SelectObject(dc, mono as _);
                SetTextColor(dc, colorref(if v.state == State::Saving { PROVISIONAL } else { FG2 }));
                let mut secs_rect = RECT { left: 0, top: 0, right: r.right - px(PAD), bottom: r.bottom };
                let mut s = wide(&s);
                DrawTextW(dc, s.as_mut_ptr(), -1, &mut secs_rect, DT_RIGHT | DT_VCENTER | DT_SINGLELINE);
                SelectObject(dc, status as _);
                DeleteObject(mono as _);
            }
            SelectObject(dc, old_font);
            DeleteObject(status as _);
        });
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        DeleteObject(bg as _);
        EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn on_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                paint(hwnd);
                0
            }
            WM_TIMER if wparam == TIMER_HIDE => {
                KillTimer(hwnd, TIMER_HIDE);
                KillTimer(hwnd, TIMER_PULSE);
                ShowWindow(hwnd, SW_HIDE);
                0
            }
            WM_TIMER if wparam == TIMER_PULSE => {
                VIEW.with(|v| {
                    let mut v = v.borrow_mut();
                    v.step = (v.step + 1) % PULSE_STEPS;
                });
                InvalidateRect(hwnd, std::ptr::null(), 0);
                0
            }
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_breathing_dot_runs_from_the_accent_down_and_back() {
        assert_eq!(breath(0), ACCENT, "the cycle starts lit");
        assert_eq!(breath(PULSE_STEPS), ACCENT);
        let dimmest = breath(PULSE_STEPS / 2);
        assert!(dimmest != ACCENT && dimmest != BG, "{dimmest:06X}");
    }

    #[test]
    fn colorref_is_bgr() {
        assert_eq!(colorref(0x112233), 0x332211);
    }

    /// The window must be click-through, never activated, and out of every screen capture
    /// from the start: styles set after showing let the first frame into a clip.
    #[test]
    fn the_banner_is_styled_before_it_is_ever_shown() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GetWindowDisplayAffinity, GetWindowLongPtrW, IsWindowVisible};
        let banner = Banner::new();
        unsafe {
            assert_eq!(IsWindowVisible(banner.hwnd), 0);
            let ex = GetWindowLongPtrW(banner.hwnd, GWL_EXSTYLE) as u32;
            for flag in [WS_EX_TRANSPARENT, WS_EX_TOOLWINDOW, WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_EX_LAYERED] {
                assert_ne!(ex & flag, 0, "missing extended style {flag:#x}");
            }
            let mut affinity = 0;
            GetWindowDisplayAffinity(banner.hwnd, &mut affinity);
            assert_eq!(affinity, WDA_EXCLUDEFROMCAPTURE);
        }
    }
}
