#![windows_subsystem = "windows"]

mod folders;
mod logging;
mod session;
mod single_instance;
mod tray;
mod winutil;

use clip_recorder::config::Config;
use clip_recorder::paths;
use logging::log;
use std::cell::RefCell;
use tray::{Action, Tray};
use winutil::wide;

thread_local! {
    /// UI-thread state. Everything that touches it runs on the Slint event loop.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

struct App {
    config: Config,
    _tray: Tray,
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
    let config = Config::load(&paths::config_file());
    session::listen();
    let tray = match Tray::new(&config.hotkey, on_action) {
        Ok(tray) => tray,
        Err(e) => {
            log(&format!("tray icon failed: {e}"));
            return;
        }
    };
    APP.with(|app| *app.borrow_mut() = Some(App { config, _tray: tray }));
    if let Err(e) = slint::run_event_loop_until_quit() {
        log(&format!("event loop failed: {e}"));
    }
    log("Clip Recorder exiting");
}

fn on_action(action: Action) {
    match action {
        Action::Quit => {
            let _ = slint::quit_event_loop();
        }
        Action::OpenFolder => {
            let folder = APP.with(|app| app.borrow().as_ref().map(|a| folders::clips(&a.config.output_folder)));
            if let Some(folder) = folder {
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
