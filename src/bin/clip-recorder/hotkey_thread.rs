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

/// Whether a message on the hotkey thread is a press of our hotkey.
fn is_press(message: u32, wparam: usize) -> bool {
    message == WM_HOTKEY && wparam == ID as usize
}

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
        let _ = tx.send(if ok {
            Ok(GetCurrentThreadId())
        } else {
            Err(GetLastError())
        });
        if !ok {
            return;
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if is_press(msg.message, msg.wParam) {
                let _ = slint::invoke_from_event_loop(on_press);
            }
        }
        UnregisterHotKey(std::ptr::null_mut(), ID);
    });
    // The thread answers before anything else: a timeout here once left a detached thread
    // holding a hotkey nobody could unregister.
    match rx.recv() {
        Ok(Ok(thread_id)) => Ok(HotkeyThread {
            thread_id,
            handle: Some(handle),
        }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use clip_recorder::hotkey::{MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT};

    #[test]
    fn only_our_hotkey_message_is_a_press() {
        assert!(is_press(WM_HOTKEY, ID as usize));
        assert!(!is_press(WM_HOTKEY, ID as usize + 1), "another hotkey id");
        assert!(!is_press(WM_HOTKEY + 1, ID as usize), "another message");
    }

    #[test]
    fn a_taken_hotkey_is_refused_with_its_error_and_freed_on_drop() {
        let key = Hotkey {
            modifiers: MOD_NOREPEAT | MOD_CONTROL | MOD_ALT | MOD_SHIFT,
            vk: 0x7A,
        }; // F11
        let first = register(key, || {}).expect("an unused combination registers");
        assert_eq!(
            register(key, || {}).err(),
            Some(1409),
            "ERROR_HOTKEY_ALREADY_REGISTERED"
        );
        // Any message but WM_QUIT keeps the loop, and the hotkey, alive.
        unsafe { PostThreadMessageW(first.thread_id, WM_HOTKEY, ID as usize, 0) };
        unsafe { PostThreadMessageW(first.thread_id, WM_HOTKEY + 1, 0, 0) };
        std::thread::sleep(Duration::from_millis(200));
        let again = register(key, || {});
        let refused = again.as_ref().err().copied();
        // An unexpected success is leaked, not dropped: with a broken message loop its drop
        // would hang and the failure below would never be reported.
        if let Ok(thread) = again {
            std::mem::forget(thread);
        }
        assert_eq!(refused, Some(1409), "a message other than WM_QUIT ended the thread");
        // Dropping must end the thread promptly: a message loop that ignores WM_QUIT would hang.
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(first);
            let _ = done.send(());
        });
        assert!(
            finished.recv_timeout(Duration::from_secs(3)).is_ok(),
            "the hotkey thread did not stop on drop"
        );
        assert!(register(key, || {}).is_ok(), "the dropped thread did not unregister it");
    }
}
