#![windows_subsystem = "windows"]

mod audio;
mod banner;
mod capture;
mod chimes;
mod folders;
mod hotkey_thread;
mod job;
mod logging;
mod monitors;
mod orphans;
mod save;
mod session;
mod single_instance;
mod tray;
mod winutil;

use banner::Banner;
use capture::Capture;
use chimes::Chime;
use clip_recorder::hotkey;
use clip_recorder::selection::GiveUp;
use clip_recorder::config::Config;
use clip_recorder::paths;
use logging::log;
use monitors::Screen;
use std::cell::RefCell;
use std::path::PathBuf;
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
    audio: audio::Audio,
    ffmpeg: PathBuf,
    banner: Banner,
    tray: Tray,
    _hotkey: Option<hotkey_thread::HotkeyThread>,
    _health: slint::Timer,
}

impl App {
    /// The banner goes on the captured screen: that is where the user is looking.
    fn notify(&self, state: banner::State, text: &str, seconds: Option<u32>, duration_ms: u32) {
        let index = self.config.monitor.min(self.screens.len().saturating_sub(1));
        self.banner.show(state, text, seconds, duration_ms, self.screens[index].rect);
    }
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
    let mut capture = match Capture::new(ffmpeg.clone()) {
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
    let mut audio = audio::Audio::new(&config.loopback_device, &config.mic_device);
    audio.start();
    let hotkey = register_hotkey(&config.hotkey, &tray);
    let health = slint::Timer::default();
    health.start(slint::TimerMode::Repeated, HEALTH_TICK, check_health);
    let banner = Banner::new();
    APP.with(|app| *app.borrow_mut() = Some(App { config, screens, capture, audio, ffmpeg, banner, tray, _hotkey: hotkey, _health: health }));
    if let Err(e) = slint::run_event_loop_until_quit() {
        log(&format!("event loop failed: {e}"));
    }
    with_app(|app| {
        app.audio.stop();
        app.capture.cleanup();
    });
    log("Clip Recorder exiting");
}

/// A dead ffmpeg is started again a second later; a dead audio stream is reopened.
fn check_health() {
    let died = with_app(|app| app.capture.has_died()).unwrap_or(false);
    if died {
        log("capture ffmpeg died: restarting it");
        slint::Timer::single_shot(Duration::from_secs(1), || {
            with_app(|app| app.capture.start(&app.config, &app.screens));
        });
        return;
    }
    with_app(|app| app.audio.heal());
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
        Action::Save => save_clip(),
        Action::Settings => {}
    }
}

/// The configured hotkey, else Ctrl+Alt+R. config.json is never written here: a hotkey that
/// failed to register must stay "changed" for Settings, or the user could never retry it.
fn register_hotkey(configured: &str, tray: &Tray) -> Option<hotkey_thread::HotkeyThread> {
    let wanted = hotkey::parse(configured);
    match hotkey_thread::register(wanted, save_clip) {
        Ok(thread) => return Some(thread),
        Err(e) => log(&format!("hotkey {configured} not registered (error {e})")),
    }
    if wanted == hotkey::DEFAULT {
        return None;
    }
    match hotkey_thread::register(hotkey::DEFAULT, save_clip) {
        Ok(thread) => {
            tray.set_hotkey(clip_recorder::config::DEFAULT_HOTKEY);
            Some(thread)
        }
        Err(e) => {
            log(&format!("fallback hotkey not registered either (error {e})"));
            None
        }
    }
}

fn save_clip() {
    with_app(|app| {
        chimes::play(Chime::Start);
        app.notify(banner::State::Saving, "Saving clip", Some(app.config.buffer_seconds), banner::SAVING_MS);
        let folder = folders::clips(&app.config.output_folder);
        save::start(&app.capture, &app.audio, &app.ffmpeg, app.config.buffer_seconds, &folder, on_saved);
    });
}

fn on_saved(outcome: save::Outcome) {
    let (chime, state, text, seconds) = match outcome {
        Ok(seconds) => (Chime::Saved, banner::State::Saved, "Clip saved", Some(seconds)),
        Err(GiveUp::TooSoon) => (Chime::Failed, banner::State::Failed, "Not enough footage yet", None),
        Err(GiveUp::Error) => (Chime::Failed, banner::State::Failed, "Clip not saved", None),
    };
    chimes::play(chime);
    with_app(|app| app.notify(state, text, seconds, banner::HOLD_MS));
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
