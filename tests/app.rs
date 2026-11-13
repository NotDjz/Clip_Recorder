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
    let dir = data_dir("save");
    let clips = dir.join("clips");
    std::fs::create_dir_all(&dir).unwrap();
    let config = serde_json::json!({
        "monitor": 1, "fps": 30, "buffer_seconds": 15,
        "output_folder": clips, "hotkey": "Ctrl+Alt+Shift+F12",
    });
    std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
    let log = || std::fs::read_to_string(dir.join("clip_recorder.log")).unwrap_or_default();
    let _app = launch(&dir);
    wait_until(Duration::from_secs(20), || ffmpeg_pid(&dir)).expect("the capture never started");
    std::thread::sleep(Duration::from_secs(5)); // past "too soon", with a few complete segments
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
        .find(|p| p.extension().is_some_and(|x| x == "mp4"));
    let clip = clip.expect("no MP4 in the clips folder");
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
