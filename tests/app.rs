//! Behaviour of the real clip-recorder.exe process (started briefly, with its own data folder).

use std::process::{Child, Command};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ENDSESSION_CLOSEAPP, EnumWindows, GetClassNameW, GetWindowThreadProcessId, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
    WM_ENDSESSION, WM_QUERYENDSESSION,
};

/// The hidden top-level window the app keeps so Restart Manager and logoff can reach it.
const SESSION_CLASS: &str = "ClipRecorderSession";

struct Search {
    pid: u32,
    found: HWND,
}

unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> i32 {
    let search = unsafe { &mut *(data as *mut Search) };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    let mut class = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) } as usize;
    if pid == search.pid && String::from_utf16_lossy(&class[..n]) == SESSION_CLASS {
        search.found = hwnd;
        return 0;
    }
    1
}

fn session_window(pid: u32) -> Option<HWND> {
    let mut search = Search { pid, found: std::ptr::null_mut() };
    unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
    (!search.found.is_null()).then_some(search.found)
}

fn send(hwnd: HWND, msg: u32, wparam: usize) {
    let mut result = 0;
    unsafe { SendMessageTimeoutW(hwnd, msg, wparam, ENDSESSION_CLOSEAPP as LPARAM, SMTO_ABORTIFHUNG, 1000, &mut result) };
}

fn wait_until<T>(limit: Duration, mut check: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Some(value) = check() {
            return Some(value);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// A data folder of its own also gives the run its own single-instance mutex, so these tests
/// work while an installed Clip Recorder is running.
fn data_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("crtest-app-{}-{name}", std::process::id()))
}

fn launch(dir: &std::path::Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_clip-recorder")).env("CLIPREC_DATA_DIR", dir).spawn().unwrap()
}

fn stop(mut app: Child) {
    let _ = app.kill();
    let _ = app.wait();
}

/// Restart Manager (MSI upgrades) and logoff close apps with WM_ENDSESSION. A tray app has no
/// visible window, so without a reachable top-level window an upgrade asks for a reboot.
#[test]
fn quits_when_windows_ends_the_session() {
    let mut app = launch(&data_dir("endsession"));
    let pid = app.id();
    let hwnd = wait_until(Duration::from_secs(10), || session_window(pid)).expect("no session window");
    let exit = wait_until(Duration::from_secs(5), || {
        send(hwnd, WM_QUERYENDSESSION, 0);
        send(hwnd, WM_ENDSESSION, 1);
        app.try_wait().unwrap()
    });
    if exit.is_none() {
        stop(app);
    }
    assert!(exit.is_some_and(|status| status.success()), "clip-recorder ignored WM_ENDSESSION");
}

/// Two instances would fight over the capture, the hotkey and NVENC sessions.
#[test]
fn a_second_launch_exits_and_the_first_keeps_running() {
    let dir = data_dir("single");
    let mut first = launch(&dir);
    wait_until(Duration::from_secs(10), || session_window(first.id())).expect("first instance never started");
    let mut second = launch(&dir);
    let exit = wait_until(Duration::from_secs(5), || second.try_wait().unwrap());
    let first_alive = first.try_wait().unwrap().is_none();
    stop(second);
    stop(first);
    assert!(exit.is_some_and(|s| s.success()), "the second launch kept running");
    assert!(first_alive, "the first instance died");
}
