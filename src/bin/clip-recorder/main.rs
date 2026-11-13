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
mod settings_window;
mod single_instance;
mod startup;
mod tray;
mod winutil;

use banner::Banner;
use capture::Capture;
use chimes::Chime;
use clip_recorder::config::Config;
use clip_recorder::hotkey;
use clip_recorder::paths;
use clip_recorder::selection::GiveUp;
use logging::log;
use monitors::Screen;
use slint::ComponentHandle;
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
    hotkey: Option<hotkey_thread::HotkeyThread>,
    settings: Option<settings_window::SettingsWindow>,
    _health: slint::Timer,
}

impl App {
    /// The banner goes on the captured screen: that is where the user is looking.
    fn notify(&self, state: banner::State, text: &str, seconds: Option<u32>, duration_ms: u32) {
        let index = self.config.monitor.min(self.screens.len().saturating_sub(1));
        self.banner
            .show(state, text, seconds, duration_ms, self.screens[index].rect);
    }
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|app| app.borrow_mut().as_mut().map(f))
}

fn main() {
    logging::install_panic_hook();
    unsafe {
        use windows_sys::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
        };
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    if !single_instance::acquire() {
        session::ask_running_instance_for_settings();
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
    session::listen(open_settings);
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
    // At sign-in the GPU driver or DXGI may not be ready for the launch probe.
    slint::Timer::single_shot(Duration::from_secs(30), || {
        with_app(|app| {
            if app.capture.reprobe() {
                app.capture.restart_video(&app.config, &app.screens);
            }
        });
    });
    let health = slint::Timer::default();
    health.start(slint::TimerMode::Repeated, HEALTH_TICK, check_health);
    let banner = Banner::new();
    APP.with(|app| {
        *app.borrow_mut() = Some(App {
            config,
            screens,
            capture,
            audio,
            ffmpeg,
            banner,
            tray,
            hotkey,
            settings: None,
            _health: health,
        })
    });
    if let Err(e) = slint::run_event_loop_until_quit() {
        log(&format!("event loop failed: {e}"));
    }
    // Torn down here, in order, rather than by thread-local destructors at process exit.
    if let Some(mut app) = APP.with(|app| app.borrow_mut().take()) {
        app.audio.stop();
        app.capture.cleanup();
        drop(app);
    }
    log("Clip Recorder exiting");
}

/// A dead ffmpeg is started again a second later; a dead audio stream is reopened.
fn check_health() {
    // Never return before healing the audio: a capture stuck restarting once stopped a dead
    // loopback from ever coming back.
    if with_app(|app| app.capture.has_died()).unwrap_or(false) {
        log("capture ffmpeg died: restarting it");
        slint::Timer::single_shot(Duration::from_secs(1), || {
            with_app(|app| app.capture.start(&app.config, &app.screens));
        });
    }
    with_app(|app| {
        app.audio.heal();
        let recording = app.capture.is_running();
        if let Some(w) = &app.settings {
            w.set_recording(recording);
        }
    });
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
        Action::Settings => open_settings(),
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

/// Each open shows the config as it is now and the devices plugged in now.
fn open_settings() {
    let shown = with_app(|app| {
        if app.settings.is_none() {
            let w = match settings_window::SettingsWindow::new() {
                Ok(w) => w,
                Err(e) => {
                    log(&format!("settings window failed: {e}"));
                    return false;
                }
            };
            w.on_save(apply_settings);
            w.on_browse(browse_folder);
            w.on_key_combination(|text, ctrl, alt, shift| {
                hotkey::format(ctrl, alt, shift, &settings_window::key_name(&text))
                    .unwrap_or_default()
                    .into()
            });
            app.settings = Some(w);
        }
        let w = app.settings.as_ref().expect("just made");
        let hw = settings_window::Hardware {
            encoder: app.capture.encoder(),
            grabber: app.capture.grabber(),
        };
        // From a dev build the Run value would start target\debug: only the installed app offers it.
        let startup = (!cfg!(debug_assertions)).then(startup::enabled);
        settings_window::fill(w, &app.config, &app.screens, &hw, audio::list_devices(), startup);
        w.set_recording(app.capture.is_running());
        w.show().is_ok()
    });
    if shown == Some(true) {
        adopt_settings_window(10);
    }
}

/// winit makes the native window a moment after show(): retried until it exists.
fn adopt_settings_window(tries: u32) {
    let done = with_app(|app| app.settings.as_ref().and_then(settings_window::adopt)).flatten();
    match done {
        Some(hwnd) => unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd);
        },
        None if tries > 0 => {
            slint::Timer::single_shot(Duration::from_millis(50), move || adopt_settings_window(tries - 1))
        }
        None => {}
    }
}

fn browse_folder() {
    let owner = with_app(|app| app.settings.as_ref().and_then(settings_window::adopt)).flatten();
    if let Some(folder) = folders::pick(owner.unwrap_or(std::ptr::null_mut()), "Save clips to")
        && let Some(Some(w)) = with_app(|app| app.settings.as_ref().map(|w| w.as_weak()))
        && let Some(w) = w.upgrade()
    {
        w.set_folder(folder.into());
    }
}

/// Save: apply what changed, then persist. Audio restarts only for a device, video only for
/// monitor/fps/buffer, the hotkey only if it registers (else config.json keeps the old one).
fn apply_settings() {
    with_app(|app| {
        let Some(w) = app.settings.as_ref().map(|w| w.clone_strong()) else {
            return;
        };
        let mut new = settings_window::read(&w, &app.config);
        let changes = clip_recorder::settings::changes(&app.config, &new);
        let mut problems = Vec::new();
        if changes.hotkey {
            app.hotkey = None; // let go of the old one first
            let registered = hotkey_thread::register(hotkey::parse(&new.hotkey), save_clip);
            if let Err(e) = &registered {
                log(&format!("hotkey {} not registered (error {e})", new.hotkey));
                problems.push(format!(
                    "{} is taken by another app: the hotkey stays {}.",
                    new.hotkey, app.config.hotkey
                ));
            }
            new.hotkey =
                clip_recorder::settings::hotkey_after_save(&app.config.hotkey, &new.hotkey, registered.is_ok())
                    .to_string();
            w.set_hotkey(new.hotkey.as_str().into());
            app.tray.set_hotkey(&new.hotkey);
            app.hotkey = match registered {
                Ok(thread) => Some(thread),
                Err(_) => register_hotkey(&new.hotkey, &app.tray),
            };
        }
        app.config = new;
        if changes.audio {
            app.audio.reload(&app.config.loopback_device, &app.config.mic_device);
        }
        if changes.video {
            app.capture.restart_video(&app.config, &app.screens);
        }
        // Re-pointed whenever it is ticked: an older install's path would start nothing.
        let startup =
            clip_recorder::settings::startup_change(w.get_startup_available(), w.get_startup(), startup::enabled());
        if let Some(on) = startup
            && !startup::set(on)
        {
            problems.push("Start with Windows could not be changed.".into());
        }
        if let Err(e) = app.config.save(&paths::config_file()) {
            log(&format!("config not saved: {e}"));
            problems.push(format!("Settings not saved: {e}"));
        }
        log(&format!("settings saved: {changes:?}"));
        w.set_problem(problems.join("\n").into());
    });
}

/// What the user hears and sees at each step of a save.
#[derive(Debug, PartialEq)]
struct Feedback {
    chime: Chime,
    state: banner::State,
    text: &'static str,
    seconds: Option<u32>,
    hold_ms: u32,
}

enum SaveEvent {
    /// The press: acknowledged at once, the length being saved shown as provisional.
    Pressed {
        buffer_seconds: u32,
    },
    Ended(save::Outcome),
}

/// Every end is heard and seen: a lost clip once looked exactly like a saved one, and a press
/// made too soon is told apart from a clip that was lost.
fn feedback(event: SaveEvent) -> Feedback {
    let (chime, state, text, seconds, hold_ms) = match event {
        SaveEvent::Pressed { buffer_seconds } => (
            Chime::Start,
            banner::State::Saving,
            "Saving clip",
            Some(buffer_seconds),
            banner::SAVING_MS,
        ),
        SaveEvent::Ended(Ok(seconds)) => (
            Chime::Saved,
            banner::State::Saved,
            "Clip saved",
            Some(seconds),
            banner::HOLD_MS,
        ),
        SaveEvent::Ended(Err(GiveUp::TooSoon)) => (
            Chime::Failed,
            banner::State::Failed,
            "Not enough footage yet",
            None,
            banner::HOLD_MS,
        ),
        SaveEvent::Ended(Err(GiveUp::Error)) => (
            Chime::Failed,
            banner::State::Failed,
            "Clip not saved",
            None,
            banner::HOLD_MS,
        ),
    };
    Feedback {
        chime,
        state,
        text,
        seconds,
        hold_ms,
    }
}

fn tell(app: &App, f: Feedback) {
    chimes::play(f.chime);
    app.notify(f.state, f.text, f.seconds, f.hold_ms);
}

fn save_clip() {
    with_app(|app| {
        tell(
            app,
            feedback(SaveEvent::Pressed {
                buffer_seconds: app.config.buffer_seconds,
            }),
        );
        let folder = folders::clips(&app.config.output_folder);
        save::start(
            &app.capture,
            &app.audio,
            &app.ffmpeg,
            app.config.buffer_seconds,
            &folder,
            on_saved,
        );
    });
}

fn on_saved(outcome: save::Outcome) {
    with_app(|app| tell(app, feedback(SaveEvent::Ended(outcome))));
}

fn open(target: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            wide("open").as_ptr(),
            wide(target).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

fn error_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide(text).as_ptr(),
            wide("Clip Recorder").as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_end_of_a_save_is_heard_and_seen() {
        let pressed = feedback(SaveEvent::Pressed { buffer_seconds: 30 });
        assert_eq!(
            (pressed.chime, pressed.state, pressed.seconds),
            (Chime::Start, banner::State::Saving, Some(30))
        );
        let saved = feedback(SaveEvent::Ended(Ok(31)));
        assert_eq!(
            (saved.chime, saved.state, saved.seconds),
            (Chime::Saved, banner::State::Saved, Some(31))
        );
        let soon = feedback(SaveEvent::Ended(Err(GiveUp::TooSoon)));
        let lost = feedback(SaveEvent::Ended(Err(GiveUp::Error)));
        for f in [&soon, &lost] {
            assert_eq!(
                (f.chime, f.state),
                (Chime::Failed, banner::State::Failed),
                "a lost clip must not look saved"
            );
        }
        assert_ne!(soon.text, lost.text, "too soon must read differently from a lost clip");
    }
}
