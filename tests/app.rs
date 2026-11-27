//! Behaviour of the real clip-recorder.exe process (started briefly, with its own data folder).

use std::process::{Child, Command};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ENDSESSION_CLOSEAPP, EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_ENDSESSION, WM_QUERYENDSESSION,
};

/// The hidden top-level window the app keeps so Restart Manager and logoff can reach it.
const SESSION_CLASS: &str = "ClipRecorderSession";

struct Search {
    pid: u32,
    class: Option<&'static str>,
    title: Option<&'static str>,
    found: HWND,
}

unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> i32 {
    let search = unsafe { &mut *(data as *mut Search) };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    let mut class = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) } as usize;
    let mut title = [0u16; 128];
    let t = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) } as usize;
    let matches = |want: Option<&str>, got: &[u16]| want.is_none_or(|w| String::from_utf16_lossy(got) == w);
    let visible = search.title.is_none() || unsafe { IsWindowVisible(hwnd) } != 0;
    if pid == search.pid && matches(search.class, &class[..n]) && matches(search.title, &title[..t]) && visible {
        search.found = hwnd;
        return 0;
    }
    1
}

fn find_window(pid: u32, class: Option<&'static str>, title: Option<&'static str>) -> Option<HWND> {
    let mut search = Search {
        pid,
        class,
        title,
        found: std::ptr::null_mut(),
    };
    unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
    (!search.found.is_null()).then_some(search.found)
}

fn session_window(pid: u32) -> Option<HWND> {
    find_window(pid, Some(SESSION_CLASS), None)
}

fn send(hwnd: HWND, msg: u32, wparam: usize) {
    let mut result = 0;
    unsafe {
        SendMessageTimeoutW(
            hwnd,
            msg,
            wparam,
            ENDSESSION_CLOSEAPP as LPARAM,
            SMTO_ABORTIFHUNG,
            1000,
            &mut result,
        )
    };
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

/// The app under test. Killed when dropped, so a failing assertion never leaves it running
/// (and never leaves cargo waiting on a pipe it inherited).
struct App(Child);

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl std::ops::Deref for App {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for App {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

fn launch(dir: &std::path::Path) -> App {
    use std::process::Stdio;
    App(Command::new(env!("CARGO_BIN_EXE_clip-recorder"))
        .env("CLIPREC_DATA_DIR", dir)
        .env(
            "CLIPREC_FFMPEG",
            concat!(env!("CARGO_MANIFEST_DIR"), r"\target\ffmpeg\ffmpeg.exe"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap())
}

/// The pid the app logged for its capture ffmpeg.
fn ffmpeg_pid(dir: &std::path::Path) -> Option<u32> {
    let log = std::fs::read_to_string(dir.join("clip_recorder.log")).ok()?;
    let line = log.lines().rev().find(|l| l.contains("capture started"))?;
    line.split("pid=").nth(1)?.split_whitespace().next()?.parse().ok()
}

fn process_exits_within(pid: u32, ms: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return true; // already gone
        }
        let exited = WaitForSingleObject(h, ms) == WAIT_OBJECT_0;
        CloseHandle(h);
        exited
    }
}

/// A crash or a Task Manager kill skips every cleanup. The kill-on-close job object must take
/// the capture ffmpeg along: an orphan keeps writing to %TEMP% and holds one of the GPU's few
/// NVENC sessions, until enough orphans make every later launch encode nothing.
#[test]
fn a_force_killed_app_takes_its_ffmpeg_along() {
    let dir = data_dir("job");
    let mut app = launch(&dir);
    let pid = wait_until(Duration::from_secs(20), || ffmpeg_pid(&dir)).expect("the capture never started");
    assert!(
        !process_exits_within(pid, 500),
        "ffmpeg died on its own: the test proves nothing"
    );
    app.kill().unwrap(); // TerminateProcess, no cleanup at all
    let _ = app.wait();
    let died = process_exits_within(pid, 3000);
    if !died {
        kill(pid); // an orphan would keep capturing, and keep cargo waiting on its pipe
    }
    assert!(died, "ffmpeg pid {pid} outlived the app");
}

fn kill(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !h.is_null() {
            TerminateProcess(h, 1);
            CloseHandle(h);
        }
    }
}

/// Restart Manager (MSI upgrades) and logoff close apps with WM_ENDSESSION. A tray app has no
/// visible window, so without a reachable top-level window an upgrade asks for a reboot.
#[test]
fn quits_when_windows_ends_the_session() {
    let dir = data_dir("endsession");
    let mut app = launch(&dir);
    let pid = app.id();
    let hwnd = wait_until(Duration::from_secs(10), || session_window(pid)).expect("no session window");
    let asked = Instant::now();
    let exit = wait_until(Duration::from_secs(5), || {
        send(hwnd, WM_QUERYENDSESSION, 0);
        send(hwnd, WM_ENDSESSION, 1);
        app.try_wait().unwrap()
    });
    assert!(
        exit.is_some_and(|status| status.success()),
        "clip-recorder ignored WM_ENDSESSION for {:?} ({exit:?}); its log:\n{}",
        asked.elapsed(),
        std::fs::read_to_string(dir.join("clip_recorder.log")).unwrap_or_default()
    );
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
    assert!(exit.is_some_and(|s| s.success()), "the second launch kept running");
    assert!(first_alive, "the first instance died");
}

/// A tray app shows nothing when its Start Menu shortcut is clicked again; Settings is the
/// window there is, so a second launch opens it in the running instance.
#[test]
fn a_second_launch_opens_settings_in_the_first() {
    let dir = data_dir("reopen");
    let first = launch(&dir);
    wait_until(Duration::from_secs(10), || session_window(first.id())).expect("first instance never started");
    assert!(find_window(first.id(), None, Some("Clip Recorder Settings")).is_none());
    let _second = launch(&dir);
    let shown = wait_until(Duration::from_secs(10), || {
        find_window(first.id(), None, Some("Clip Recorder Settings"))
    });
    assert!(shown.is_some(), "no Settings window");
}

fn press(keys: &[u16]) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    };
    let key = |vk: u16, flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let mut inputs: Vec<INPUT> = keys.iter().map(|&k| key(k, 0)).collect();
    inputs.extend(keys.iter().rev().map(|&k| key(k, KEYEVENTF_KEYUP)));
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
}

/// The whole path a user takes: a hotkey press ends as a decodable MP4 in the clips folder.
/// It captures the second screen and plays the chimes, like the real thing. The hotkey is one
/// nobody uses, so an installed Clip Recorder holding Ctrl+Alt+R is left alone.
#[test]
fn the_hotkey_saves_a_clip_that_decodes() {
    hotkey_save_decodes("save", "mp4", false);
}

/// The same through the real ffmpeg into Matroska: a TS-to-MKV copy it refused would turn every
/// MKV save into "Clip not saved", with the fake ffmpeg's tests still green.
#[test]
fn the_hotkey_saves_an_mkv_that_decodes() {
    hotkey_save_decodes("save-mkv", "mkv", false);
}

/// League of Legends turns other apps' registered hotkeys off while it has the foreground
/// (measured in a game: RegisterHotKey 0 presses in 3, a keyboard hook 3 in 3). The press is
/// made with a window doing the same in front.
#[test]
fn the_hotkey_saves_while_a_game_turns_hotkeys_off() {
    hotkey_save_decodes("save-nohotkeys", "mp4", true);
}

/// The top-left corner of a screen that is not the primary (the primary's on a single screen).
fn second_screen_corner() -> (i32, i32) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
    unsafe extern "system" fn visit(screen: HMONITOR, _: HDC, _: *mut RECT, found: LPARAM) -> i32 {
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(screen, &mut info) } != 0 && info.dwFlags & MONITORINFOF_PRIMARY == 0 {
            unsafe { *(found as *mut (i32, i32)) = (info.rcMonitor.left, info.rcMonitor.top) };
            return 0;
        }
        1
    }
    let mut corner = (0, 0);
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(visit),
            &mut corner as *mut (i32, i32) as LPARAM,
        )
    };
    corner
}

/// A window that reads the keyboard as raw input with RIDEV_NOHOTKEYS, in the foreground, on a
/// thread of its own that pumps its messages. Closed when dropped.
struct Game {
    thread: u32,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Game {
    fn in_front() -> Game {
        use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
        use windows_sys::Win32::UI::Input::{RAWINPUTDEVICE, RIDEV_NOHOTKEYS, RegisterRawInputDevices};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, DispatchMessageW, GetForegroundWindow, GetMessageW, MSG,
            SetForegroundWindow, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || unsafe {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let none = std::ptr::null_mut();
            // 1 px on the second screen: it takes the foreground without covering anything.
            let (x, y) = second_screen_corner();
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                1,
                1,
                none,
                none,
                none,
                std::ptr::null(),
            );
            let keyboard = RAWINPUTDEVICE {
                usUsagePage: 1,
                usUsage: 6,
                dwFlags: RIDEV_NOHOTKEYS,
                hwndTarget: hwnd,
            };
            let raw = RegisterRawInputDevices(&keyboard, 1, size_of::<RAWINPUTDEVICE>() as u32) != 0;
            let me = GetCurrentThreadId();
            let front = GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
            AttachThreadInput(front, me, 1);
            SetForegroundWindow(hwnd);
            AttachThreadInput(front, me, 0);
            let _ = tx.send((me, raw, GetForegroundWindow() == hwnd));
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                DispatchMessageW(&msg);
            }
            DestroyWindow(hwnd);
        });
        let (thread, raw, front) = rx.recv().unwrap();
        let game = Game {
            thread,
            handle: Some(handle),
        };
        assert!(raw, "RegisterRawInputDevices failed");
        assert!(
            front,
            "the game window did not get the foreground: the press would prove nothing"
        );
        game
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        unsafe { PostThreadMessageW(self.thread, WM_QUIT, 0, 0) };
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn hotkey_save_decodes(name: &str, format: &str, game_in_front: bool) {
    let dir = data_dir(name);
    let clips = dir.join("clips");
    std::fs::create_dir_all(&dir).unwrap();
    let config = serde_json::json!({
        "monitor": 1, "fps": 30, "buffer_seconds": 15,
        "output_folder": clips, "hotkey": "Ctrl+Alt+Shift+F12", "format": format,
    });
    std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
    let log = || std::fs::read_to_string(dir.join("clip_recorder.log")).unwrap_or_default();
    let _app = launch(&dir);
    wait_until(Duration::from_secs(20), || ffmpeg_pid(&dir)).expect("the capture never started");
    std::thread::sleep(Duration::from_secs(5)); // past "too soon", with a few complete segments
    let _game = game_in_front.then(Game::in_front);
    press(&[0x11, 0x12, 0x10, 0x7B]); // Ctrl, Alt, Shift, F12
    let done = wait_until(Duration::from_secs(40), || {
        log().lines().find(|l| l.contains("done: saved=")).map(String::from)
    });
    let done = done.unwrap_or_else(|| panic!("no save finished:\n{}", log()));
    assert!(done.contains("saved=true"), "{}", log());
    // The clip is placed by ffmpeg's capture clock, parsed from -debug_ts text: an ffmpeg
    // upgrade that rewords it would silently put every clip back on file times.
    assert!(log().contains("anchor=capture"), "{}", log());
    let clip = std::fs::read_dir(&clips)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == format));
    let clip = clip.unwrap_or_else(|| panic!("no .{format} in the clips folder"));
    let decode = Command::new(concat!(env!("CARGO_MANIFEST_DIR"), r"\target\ffmpeg\ffmpeg.exe"))
        .args(["-v", "error", "-i"])
        .arg(&clip)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    assert!(
        decode.status.success() && decode.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&decode.stderr)
    );
}
