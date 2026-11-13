//! The Settings window. It shows the config and reads the form back into a new Config; what a
//! Save then restarts is decided in clip_recorder::settings and applied by main.rs.

use crate::monitors::Screen;
use clip_recorder::config::{BUFFER_OPTIONS, Config};
use clip_recorder::ffmpeg_cmd::{Encoder, Grabber};
use clip_recorder::settings::{device_at, device_choices, fps_choices, index_of};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

slint::slint! {
    import { Button, ComboBox, LineEdit, CheckBox, Palette } from "std-widgets.slint";

    global Ink {
        out property <color> bg: #191C1B;
        out property <color> bg2: #232725;
        out property <color> bg3: #2F3532;
        out property <color> fg: #E6EAE7;
        out property <color> fg2: #8B958F;
        // The record dot and Save. Nothing else.
        out property <color> accent: #FF4A3D;
        out property <color> tick: #49524D;
        out property <color> kept: #2C2422;
        // Save's fill: the accent darkened until white text on it reaches 4.66:1 (WCAG AA).
        out property <color> save: #D63A2F;
    }

    // Save: reachable with Tab, fired by Enter or Space, focus shown by a ring (WCAG 2.2 AA).
    component AccentButton inherits Rectangle {
        in property <string> text;
        callback clicked;
        width: 130px;
        height: 32px;
        border-radius: 4px;
        background: touch.pressed ? Ink.save.darker(15%) : touch.has-hover ? Ink.save.brighter(8%) : Ink.save;
        border-width: focus.has-focus ? 2px : 0px;
        border-color: Ink.fg;
        accessible-role: button;
        accessible-label: text;
        accessible-action-default => { clicked(); }
        Text { text: root.text; color: #FFFFFF; font-weight: 700; }
        touch := TouchArea {
            clicked => {
                focus.focus();
                root.clicked();
            }
        }
        focus := FocusScope {
            // Without this the scope takes the first press to focus itself, and Save needed
            // two clicks: the TouchArea below focuses it instead.
            focus-on-click: false;
            key-pressed(event) => {
                if event.text == Key.Return || event.text == " " {
                    root.clicked();
                    return accept;
                }
                return reject;
            }
        }
    }

    component Group inherits VerticalLayout {
        in property <string> title;
        spacing: 4px;
        Text { text: title; color: Ink.fg2; font-size: 12px; }
        Rectangle {
            background: Ink.bg2;
            border-radius: 4px;
            VerticalLayout {
                padding: 10px;
                spacing: 6px;
                @children
            }
        }
    }

    component Row inherits HorizontalLayout {
        in property <string> label;
        spacing: 8px;
        Text { text: label; color: Ink.fg2; width: 104px; vertical-alignment: center; }
        @children
    }

    component Unit inherits Text {
        color: Ink.fg2;
        vertical-alignment: center;
    }

    export component SettingsWindow inherits Window {
        title: "Clip Recorder Settings";
        width: 470px;
        background: Ink.bg;
        default-font-family: "Segoe UI";
        default-font-size: 13px;

        in property <bool> recording;
        // Encoder and capture method: hardware facts, read out rather than offered as rows.
        in property <string> hardware;
        in property <[string]> monitors;
        in-out property <int> monitor;
        in property <[string]> fps-options;
        in-out property <int> fps;
        in property <[string]> buffer-options;
        in-out property <int> buffer;
        in property <[string]> loopbacks;
        in-out property <int> loopback;
        in property <[string]> mics;
        in-out property <int> mic;
        in-out property <string> folder;
        in-out property <string> hotkey;
        in-out property <bool> startup;
        in property <bool> startup-available;
        in-out property <string> problem;
        in-out property <bool> capturing;

        callback browse();
        callback save();
        // The combination as config.json writes it, or "" if it would not register.
        callback key-combination(string, bool, bool, bool) -> string;

        property <int> seconds: buffer-options[buffer].to-float();

        init => { Palette.color-scheme = ColorScheme.dark; }

        keys := FocusScope {
            key-pressed(event) => {
                if !capturing { return reject; }
                if event.text == Key.Escape {
                    capturing = false;
                    return accept;
                }
                let combination = key-combination(event.text, event.modifiers.control, event.modifiers.alt, event.modifiers.shift);
                if combination != "" {
                    hotkey = combination;
                    capturing = false;
                }
                return accept;
            }

            VerticalLayout {
                padding: 16px;
                spacing: 12px;

                // Status: is it running, and what does the key do?
                VerticalLayout {
                    spacing: 9px;
                    HorizontalLayout {
                        spacing: 6px;
                        Rectangle {
                            width: 9px; height: 9px; y: (parent.height - self.height) / 2;
                            border-radius: 4.5px;
                            background: recording ? Ink.accent : Ink.fg2;
                        }
                        Text {
                            text: recording ? "Recording" : "Stopped";
                            color: Ink.fg; font-size: 14px; font-weight: 700;
                        }
                        Text {
                            text: hardware + "     " + fps-options[fps] + " fps";
                            color: Ink.fg2; font-size: 11px;
                            horizontal-alignment: right; vertical-alignment: center;
                            horizontal-stretch: 1;
                        }
                    }
                    // The 120 s the app always holds, with the slice the hotkey writes in red.
                    Rectangle {
                        height: 26px;
                        background: Ink.bg2;
                        border-width: 1px; border-color: Ink.bg3;
                        Rectangle {
                            x: parent.width * (1 - seconds / 120);
                            width: parent.width * seconds / 120;
                            background: Ink.kept;
                        }
                        for i in 61: Rectangle {
                            x: parent.width - parent.width * i / 60 - 1px;
                            y: parent.height - self.height;
                            width: 1px;
                            height: mod(i, 15) == 0 ? 12px : 8px;
                            background: mod(i, 15) == 0 ? Ink.fg2 : Ink.tick;
                        }
                        Rectangle {
                            x: parent.width * (1 - seconds / 120);
                            width: 1px;
                            background: Ink.accent;
                        }
                        Rectangle { x: parent.width - 2px; width: 2px; background: Ink.accent; }
                    }
                    Text {
                        text: (capturing ? "Press the new combination" : hotkey) + " saves the last " + seconds + " seconds.";
                        color: Ink.fg;
                    }
                }

                Group {
                    title: "Capture";
                    Row { label: "Monitor"; ComboBox { model: monitors; current-index <=> monitor; } }
                    Row {
                        label: "Frame rate";
                        ComboBox { model: fps-options; current-index <=> fps; width: 90px; }
                        Unit { text: "fps"; }
                        Rectangle {}
                    }
                }

                Group {
                    title: "Audio";
                    Row { label: "System sound"; ComboBox { model: loopbacks; current-index <=> loopback; } }
                    Row { label: "Microphone"; ComboBox { model: mics; current-index <=> mic; } }
                }

                Group {
                    title: "Replay";
                    Row {
                        label: "Keep the last";
                        ComboBox { model: buffer-options; current-index <=> buffer; width: 90px; }
                        Unit { text: "seconds"; }
                        Rectangle {}
                    }
                    Row {
                        label: "Save clips to";
                        LineEdit { text <=> folder; }
                        Button { text: "Browse"; clicked => { browse(); } }
                    }
                    Row {
                        label: "Hotkey";
                        hotkey-button := Button {
                            text: capturing ? "Press a key..." : hotkey;
                            primary: capturing;
                            clicked => {
                                capturing = true;
                                keys.focus();
                            }
                        }
                        Unit { text: "click, then press a combination (Esc cancels)"; font-size: 11px; }
                        Rectangle {}
                    }
                }

                Group {
                    title: "Startup";
                    HorizontalLayout {
                        spacing: 8px;
                        CheckBox { text: "Start with Windows"; checked <=> startup; enabled: startup-available; horizontal-stretch: 0; }
                        if !startup-available: Unit { text: "installed app only"; font-size: 11px; }
                        Rectangle { horizontal-stretch: 1; }
                    }
                }

                if problem != "": Text { text: problem; color: Ink.accent; wrap: word-wrap; }

                Rectangle { height: 1px; background: Ink.bg3; }
                HorizontalLayout {
                    AccentButton { text: "Save changes"; clicked => { save(); } }
                    Rectangle {}
                }
            }
        }
    }
}

fn strings(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items.into_iter().map(SharedString::from).collect::<Vec<_>>(),
    ))
}

fn label(i: usize, s: &Screen) -> String {
    format!(
        "{}: {}×{}{}",
        i + 1,
        s.rect.w,
        s.rect.h,
        if s.primary { "  (primary)" } else { "" }
    )
}

pub struct Hardware {
    pub encoder: Encoder,
    pub grabber: Grabber,
}

/// Fills the form from the config. Device lists are read live, so one plugged in since launch
/// is offered.
pub fn fill(
    w: &SettingsWindow,
    config: &Config,
    screens: &[Screen],
    hw: &Hardware,
    devices: (Vec<String>, Vec<String>),
    startup: Option<bool>,
) {
    w.set_hardware(
        format!(
            "{}     {}",
            if hw.encoder == Encoder::Nvenc { "NVENC" } else { "x264" },
            if hw.grabber == Grabber::Ddagrab { "DXGI" } else { "GDI" }
        )
        .into(),
    );
    w.set_monitors(strings(screens.iter().enumerate().map(|(i, s)| label(i, s))));
    w.set_monitor(config.monitor.min(screens.len().saturating_sub(1)) as i32);
    let fps = fps_choices(hw.grabber == Grabber::Ddagrab);
    w.set_fps_options(strings(fps.iter().map(u32::to_string)));
    w.set_fps(index_of(&fps, config.fps) as i32);
    w.set_buffer_options(strings(BUFFER_OPTIONS.iter().map(u32::to_string)));
    w.set_buffer(index_of(&BUFFER_OPTIONS, config.buffer_seconds) as i32);
    let (loopbacks, loopback) = device_choices(devices.0, &config.loopback_device);
    w.set_loopbacks(strings(loopbacks));
    w.set_loopback(loopback as i32);
    let (mics, mic) = device_choices(devices.1, &config.mic_device);
    w.set_mics(strings(mics));
    w.set_mic(mic as i32);
    w.set_folder(
        crate::folders::clips(&config.output_folder)
            .display()
            .to_string()
            .into(),
    );
    w.set_hotkey(config.hotkey.as_str().into());
    w.set_capturing(false);
    w.set_startup_available(startup.is_some());
    w.set_startup(startup.unwrap_or(false));
    w.set_problem("".into());
}

fn picked(model: ModelRc<SharedString>, index: i32) -> String {
    let choices: Vec<String> = model.iter().map(String::from).collect();
    device_at(&choices, index.max(0) as usize)
}

/// The form as a Config. The folder is written back empty while it is the default, so the
/// default keeps following a relocated Videos folder.
pub fn read(w: &SettingsWindow, config: &Config) -> Config {
    let number = |model: ModelRc<SharedString>, index: i32, fallback: u32| {
        model
            .row_data(index.max(0) as usize)
            .and_then(|s| s.parse().ok())
            .unwrap_or(fallback)
    };
    let folder = w.get_folder().trim().to_string();
    let default = crate::folders::clips("").display().to_string();
    Config {
        monitor: w.get_monitor().max(0) as usize,
        fps: number(w.get_fps_options(), w.get_fps(), config.fps),
        buffer_seconds: number(w.get_buffer_options(), w.get_buffer(), config.buffer_seconds),
        output_folder: if folder == default || folder.is_empty() {
            String::new()
        } else {
            folder
        },
        loopback_device: picked(w.get_loopbacks(), w.get_loopback()),
        mic_device: picked(w.get_mics(), w.get_mic()),
        hotkey: w.get_hotkey().to_string(),
    }
}

/// Slint's key text for the key part of a combination: letters, digits, F1-F12.
pub fn key_name(text: &str) -> String {
    use slint::platform::Key;
    let fkeys = [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
    ];
    match fkeys.iter().position(|k| SharedString::from(*k) == text) {
        Some(i) => format!("F{}", i + 1),
        None => text.to_string(),
    }
}

/// The native window, once winit has made it: the exe's icon, and a dark title bar. Dark mode
/// alone is not enough: with "accent colour on title bars" on, the active bar takes the accent
/// and sits across the dark window like a stripe, so the caption colour is set too (Win11).
pub fn adopt(w: &SettingsWindow) -> Option<windows_sys::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let handle = w.window().window_handle();
    let RawWindowHandle::Win32(win32) = handle.window_handle().ok()?.as_raw() else {
        return None;
    };
    let hwnd = win32.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    unsafe {
        let dark: i32 = 1;
        let caption: u32 = 0x001B1C19; // Ink.bg as COLORREF (BGR)
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&dark as *const i32).cast(),
            4,
        );
        DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR as u32, (&caption as *const u32).cast(), 4);
        // A form of fixed size: no maximize box.
        SetWindowLongPtrW(
            hwnd,
            GWL_STYLE,
            GetWindowLongPtrW(hwnd, GWL_STYLE) & !(WS_MAXIMIZEBOX as isize),
        );
        // The ICO has a frame per size: load the ones this DPI needs.
        let dpi = GetDpiForWindow(hwnd);
        let module = GetModuleHandleW(std::ptr::null());
        for (which, cx, cy) in [(ICON_SMALL, SM_CXSMICON, SM_CYSMICON), (ICON_BIG, SM_CXICON, SM_CYICON)] {
            let icon = LoadImageW(
                module,
                1 as _,
                IMAGE_ICON,
                GetSystemMetricsForDpi(cx, dpi),
                GetSystemMetricsForDpi(cy, dpi),
                LR_DEFAULTCOLOR,
            );
            if !icon.is_null() {
                SendMessageW(hwnd, WM_SETICON, which as usize, icon as isize);
            }
        }
    }
    Some(hwnd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clip_recorder::ffmpeg_cmd::Monitor;
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::{Key, WindowEvent};

    fn screens() -> Vec<Screen> {
        let s = |x, primary| Screen {
            name: String::new(),
            rect: Monitor {
                x,
                y: 0,
                w: 2560,
                h: 1440,
            },
            primary,
        };
        vec![s(0, true), s(2560, false)]
    }

    /// The testing platform is set once per thread: call this once per test.
    fn window(config: &Config) -> SettingsWindow {
        let w = SettingsWindow::new().unwrap();
        let hw = Hardware {
            encoder: Encoder::Nvenc,
            grabber: Grabber::Ddagrab,
        };
        let devices = (
            vec!["Speakers (Realtek)".into(), "Headphones (USB)".into()],
            vec!["Microphone (Realtek)".into()],
        );
        fill(&w, config, &screens(), &hw, devices, Some(true));
        w.on_key_combination(|text, ctrl, alt, shift| {
            clip_recorder::hotkey::format(ctrl, alt, shift, &key_name(&text))
                .unwrap_or_default()
                .into()
        });
        w
    }

    /// Opening Settings and pressing Save, to change nothing, changes nothing: Auto stays Auto
    /// (once, a Save pinned the detected device), the default folder stays empty.
    #[test]
    fn an_untouched_form_reads_back_the_same_config() {
        i_slint_backend_testing::init_no_event_loop();
        for config in [
            Config::default(),
            Config {
                monitor: 1,
                fps: 240,
                buffer_seconds: 90,
                mic_device: "Microphone (Realtek)".into(),
                hotkey: "Ctrl+Shift+F9".into(),
                ..Config::default()
            },
            Config {
                loopback_device: "Unplugged (USB)".into(),
                output_folder: r"D:\Clips".into(),
                ..Config::default()
            },
        ] {
            assert_eq!(read(&window(&config), &config), config);
        }
    }

    #[test]
    fn picking_the_second_monitor_and_120_fps_reads_back_as_such() {
        i_slint_backend_testing::init_no_event_loop();
        let config = Config::default();
        let w = window(&config);
        w.set_monitor(1);
        w.set_fps(2);
        w.set_loopback(2);
        let monitors: Vec<String> = w.get_monitors().iter().map(String::from).collect();
        assert_eq!(monitors, ["1: 2560×1440  (primary)", "2: 2560×1440"]);
        let new = read(&w, &config);
        assert_eq!(
            (new.monitor, new.fps, new.loopback_device.as_str()),
            (1, 120, "Headphones (USB)")
        );
    }

    fn press(w: &SettingsWindow, keys: &[SharedString]) {
        for k in keys {
            w.window().dispatch_event(WindowEvent::KeyPressed { text: k.clone() });
        }
        for k in keys.iter().rev() {
            w.window().dispatch_event(WindowEvent::KeyReleased { text: k.clone() });
        }
    }

    #[test]
    fn the_hotkey_button_captures_a_combination() {
        i_slint_backend_testing::init_no_event_loop();
        let w = window(&Config::default());
        w.show().unwrap();
        let button = ElementHandle::find_by_accessible_label(&w, "Ctrl+Alt+R")
            .next()
            .expect("hotkey button");
        button.invoke_accessible_default_action();
        assert!(w.get_capturing());
        press(&w, &["r".into()]); // no modifier: ignored, still waiting
        assert!(w.get_capturing());
        press(&w, &[Key::Control.into(), Key::Shift.into(), Key::F9.into()]);
        assert_eq!((w.get_capturing(), w.get_hotkey().as_str()), (false, "Ctrl+Shift+F9"));
    }

    #[test]
    fn escape_cancels_a_capture() {
        i_slint_backend_testing::init_no_event_loop();
        let w = window(&Config::default());
        w.show().unwrap();
        ElementHandle::find_by_accessible_label(&w, "Ctrl+Alt+R")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        press(&w, &[Key::Escape.into()]);
        assert_eq!((w.get_capturing(), w.get_hotkey().as_str()), (false, "Ctrl+Alt+R"));
    }

    /// Keyboard only: Shift+Tab from the start lands on the last control, Save, and Enter fires it.
    #[test]
    fn the_save_button_works_from_the_keyboard() {
        i_slint_backend_testing::init_no_event_loop();
        let w = window(&Config::default());
        w.show().unwrap();
        let saved = std::rc::Rc::new(std::cell::Cell::new(false));
        let s = saved.clone();
        w.on_save(move || s.set(true));
        press(&w, &[Key::Shift.into(), Key::Tab.into()]);
        press(&w, &[Key::Return.into()]);
        assert!(saved.get(), "Enter on the focused Save did nothing");
    }

    /// One button, Save. A separate Apply was a confusing subset of it and was removed.
    #[test]
    fn there_is_no_apply_button() {
        i_slint_backend_testing::init_no_event_loop();
        let w = window(&Config::default());
        assert!(ElementHandle::find_by_accessible_label(&w, "Apply").next().is_none());
        assert!(
            ElementHandle::find_by_accessible_label(&w, "Save changes")
                .next()
                .is_some()
        );
    }

    /// One mouse click saves: the button's focus scope once swallowed the first click.
    #[test]
    fn one_click_on_save_saves() {
        i_slint_backend_testing::init_no_event_loop();
        let w = window(&Config::default());
        w.show().unwrap();
        let saved = std::rc::Rc::new(std::cell::Cell::new(0));
        let s = saved.clone();
        w.on_save(move || s.set(s.get() + 1));
        let save = ElementHandle::find_by_accessible_label(&w, "Save changes")
            .next()
            .unwrap();
        save.mock_single_click(slint::platform::PointerEventButton::Left);
        assert_eq!(saved.get(), 1, "the first click did not save");
    }

    #[test]
    fn the_save_button_saves() {
        i_slint_backend_testing::init_no_event_loop();
        let w = window(&Config::default());
        let saved = std::rc::Rc::new(std::cell::Cell::new(false));
        let s = saved.clone();
        w.on_save(move || s.set(true));
        ElementHandle::find_by_accessible_label(&w, "Save changes")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert!(saved.get());
    }
}
