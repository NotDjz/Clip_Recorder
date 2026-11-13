//! The global save hotkey: RegisterHotKey on a thread of its own with its message pump, each
//! press handed to the UI thread.

use clip_recorder::hotkey::Hotkey;
use std::sync::mpsc;
use std::time::Duration;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, PostThreadMessageW, WM_HOTKEY, WM_QUIT};

const ID: i32 = 1;

pub struct HotkeyThread {
    thread_id: u32,
    handle: Option<std::thread::JoinHandle<()>>,
}

/// Registers `hotkey`, running `on_press` on the UI thread for each press. Err(Win32 error) if
/// it could not be registered (1409: another app holds it): the caller must then not write it
/// to config.json, or the user could never retry it.
pub fn register(hotkey: Hotkey, on_press: fn()) -> Result<HotkeyThread, u32> {
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || unsafe {
        let mut ok = RegisterHotKey(std::ptr::null_mut(), ID, hotkey.modifiers, hotkey.vk) != 0;
        if !ok {
            // A previous instance may still be letting go of it.
            std::thread::sleep(Duration::from_millis(150));
            ok = RegisterHotKey(std::ptr::null_mut(), ID, hotkey.modifiers, hotkey.vk) != 0;
        }
        let _ = tx.send(if ok { Ok(GetCurrentThreadId()) } else { Err(GetLastError()) });
        if !ok {
            return;
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY && msg.wParam == ID as usize {
                let _ = slint::invoke_from_event_loop(on_press);
            }
        }
        UnregisterHotKey(std::ptr::null_mut(), ID);
    });
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(thread_id)) => Ok(HotkeyThread { thread_id, handle: Some(handle) }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(0),
    }
}

impl Drop for HotkeyThread {
    fn drop(&mut self) {
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
