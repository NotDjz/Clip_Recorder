#![windows_subsystem = "windows"]

mod capture;
mod folders;
mod job;
mod logging;
mod monitors;
mod orphans;
mod session;
mod single_instance;
mod tray;
mod winutil;

use capture::Capture;
use clip_recorder::config::Config;
use clip_recorder::paths;
use logging::log;
use monitors::Screen;
use std::cell::RefCell;
use std::time::Duration;
use tray::{Action, Tray};
use winutil::wide;

/// How often the capture's health is checked (and later, the audio streams healed).
const HEALTH_TICK: Duration = Duration::from_secs(2);

thread_local! {
    /// UI-thread state. Everything that touches it runs on the Slint event loop.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

struct App {
    config: Config,
    screens: Vec<Screen>,
    capture: Capture,
    _tray: Tray,
    _health: slint::Timer,
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|app| app.borrow_mut().as_mut().map(f))
}

fn main() {
    unsafe {
        use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    if !single_instance::acquire() {
        return;
    }
    log(&format!("Clip Recorder {} starting", env!("CARGO_PKG_VERSION")));
    orphans::sweep(&paths::segments_root());
    let ffmpeg = capture::ffmpeg_path();
    if !ffmpeg.is_file() {
        log(&format!("ffmpeg not found at {}", ffmpeg.display()));
        error_box("ffmpeg.exe is missing next to Clip Recorder.\n\nReinstall Clip Recorder to restore it.");
        return;
    }
    let config = Config::load(&paths::config_file());
    let screens = monitors::list();
    session::listen();
    let mut capture = match Capture::new(ffmpeg) {
        Ok(capture) => capture,
        Err(e) => {
            log(&format!("capture folder not created: {e}"));
            return;
        }
    };
    let tray = match Tray::new(&config.hotkey, on_action) {
        Ok(tray) => tray,
        Err(e) => {
            log(&format!("tray icon failed: {e}"));
            return;
        }
    };
    capture.start(&config, &screens);
    let health = slint::Timer::default();
    health.start(slint::TimerMode::Repeated, HEALTH_TICK, check_health);
    APP.with(|app| *app.borrow_mut() = Some(App { config, screens, capture, _tray: tray, _health: health }));
    if let Err(e) = slint::run_event_loop_until_quit() {
        log(&format!("event loop failed: {e}"));
    }
    with_app(|app| app.capture.cleanup());
    log("Clip Recorder exiting");
}

/// A dead ffmpeg is started again a second later.
fn check_health() {
    let died = with_app(|app| app.capture.has_died()).unwrap_or(false);
    if died {
        log("capture ffmpeg died: restarting it");
        slint::Timer::single_shot(Duration::from_secs(1), || {
            with_app(|app| app.capture.start(&app.config, &app.screens));
        });
    }
}

fn on_action(action: Action) {
    match action {
        Action::Quit => {
            let _ = slint::quit_event_loop();
        }
        Action::OpenFolder => {
            if let Some(folder) = with_app(|app| folders::clips(&app.config.output_folder)) {
                let _ = std::fs::create_dir_all(&folder);
                open(&folder.to_string_lossy());
            }
        }
        Action::Save | Action::Settings => {}
    }
}

fn open(target: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), wide("open").as_ptr(), wide(target).as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

fn error_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Clip Recorder").as_ptr(), MB_OK | MB_ICONERROR) };
}
