//! The global save hotkey: RegisterHotKey on a thread of its own with its message pump, each
//! press handed to the UI thread. Plus a low-level keyboard hook on the same thread: League of
//! Legends turns registered hotkeys off while it has the foreground (measured in a game:
//! RegisterHotKey 0 presses in 3, the hook 3 in 3). The hook eats the presses it sees, so each
//! press saves once; RegisterHotKey still claims the combination, says when another app holds
//! it, and catches the presses if Windows ever drops the hook. The hook keeps no keystroke.

use clip_recorder::hotkey::{Hotkey, Key, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN, Watch};
use std::cell::RefCell;
use std::sync::mpsc;
use std::time::Duration;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, MSG, PostThreadMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_HOTKEY, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

const ID: i32 = 1;

/// The hotkey thread's watch and what a press runs.
type Hooked = Option<(Watch, fn())>;

thread_local! {
    /// The hook runs on the thread that installed it.
    static HOOK: RefCell<Hooked> = const { RefCell::new(None) };
}

unsafe extern "system" fn on_key(code: i32, wparam: usize, lparam: isize) -> isize {
    if code == HC_ACTION as i32 {
        let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let key = HOOK.with_borrow_mut(|hook| {
            let (watch, on_press) = hook.as_mut()?;
            let key = watch.key(event.vkCode, down, held_modifiers, event.time);
            if key == Key::Press {
                let _ = slint::invoke_from_event_loop(*on_press);
            }
            Some(key)
        });
        if matches!(key, Some(Key::Press | Key::Eat)) {
            return 1;
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// The modifier keys held now, as RegisterHotKey's MOD_ flags.
fn held_modifiers() -> u32 {
    modifiers(|vk| unsafe { GetAsyncKeyState(vk as i32) } < 0)
}

fn modifiers(held: impl Fn(u16) -> bool) -> u32 {
    [
        (VK_CONTROL, MOD_CONTROL),
        (VK_MENU, MOD_ALT),
        (VK_SHIFT, MOD_SHIFT),
        (VK_LWIN, MOD_WIN),
        (VK_RWIN, MOD_WIN),
    ]
    .into_iter()
    .filter(|&(vk, _)| held(vk))
    .fold(0, |mods, (_, flag)| mods | flag)
}

/// The keyboard hook on the calling thread, which must pump messages. Null if Windows refused
/// it: the hotkey then works only where games leave registered hotkeys on, as before.
unsafe fn install_hook(hotkey: Hotkey, on_press: fn()) -> HHOOK {
    HOOK.set(Some((Watch::new(hotkey), on_press)));
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(on_key), GetModuleHandleW(std::ptr::null()), 0) };
    if hook.is_null() {
        let e = unsafe { GetLastError() };
        crate::logging::log(&format!(
            "keyboard hook not installed (error {e}): games may swallow the hotkey"
        ));
    }
    hook
}

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
        let hook = install_hook(hotkey, on_press);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if is_press(msg.message, msg.wParam) {
                let _ = slint::invoke_from_event_loop(on_press);
            }
        }
        UnhookWindowsHookEx(hook); // a no-op on the null of a hook that failed
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
    fn the_held_keys_read_as_registerhotkey_modifiers() {
        assert_eq!(modifiers(|_| false), 0);
        assert_eq!(modifiers(|vk| vk == VK_CONTROL || vk == VK_MENU), MOD_CONTROL | MOD_ALT);
        assert_eq!(modifiers(|vk| vk == VK_SHIFT), MOD_SHIFT);
        assert_eq!(modifiers(|vk| vk == VK_RWIN), MOD_WIN, "either Win key");
        assert_eq!(modifiers(|vk| vk == VK_LWIN || vk == VK_RWIN), MOD_WIN, "both Win keys");
    }

    #[test]
    fn the_hook_eats_the_hotkey_and_passes_every_other_key() {
        use windows_sys::Win32::UI::WindowsAndMessaging::WM_KEYUP;
        // F13 without a modifier (which parse() never gives): the test runs with none held.
        HOOK.set(Some((
            Watch::new(Hotkey {
                modifiers: MOD_NOREPEAT,
                vk: 0x7C,
            }),
            || {},
        )));
        let call = |code: i32, message: u32, vk: u32| {
            let event = KBDLLHOOKSTRUCT {
                vkCode: vk,
                scanCode: 0,
                flags: 0,
                time: 0,
                dwExtraInfo: 0,
            };
            unsafe { on_key(code, message as usize, &event as *const KBDLLHOOKSTRUCT as isize) }
        };
        assert_eq!(call(0, WM_KEYDOWN, 0x7C), 1, "the press is eaten");
        assert_eq!(call(0, WM_KEYDOWN, 0x7C), 1, "its auto-repeat too");
        assert_eq!(call(0, WM_KEYUP, 0x7C), 0, "the key-up goes on");
        assert_eq!(call(0, WM_KEYDOWN, 0x41), 0, "another key goes on");
        assert_eq!(
            call(-1, WM_KEYDOWN, 0x7C),
            0,
            "not HC_ACTION: Windows says not to touch it"
        );
        assert_eq!(call(0, WM_SYSKEYDOWN, 0x7C), 1, "a system key-down is a press too");
        HOOK.set(None);
        assert_eq!(call(0, WM_KEYDOWN, 0x7C), 0, "no hotkey on this thread");
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
