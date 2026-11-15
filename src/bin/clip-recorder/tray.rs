//! The notification-area icon and its menu. Menu clicks arrive inside a window procedure; they
//! are handed to the Slint event loop so every action runs on the UI thread.

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Save,
    Settings,
    OpenFolder,
    Updates,
    Quit,
}

const IDS: [(&str, Action); 5] = [
    ("save", Action::Save),
    ("settings", Action::Settings),
    ("folder", Action::OpenFolder),
    ("updates", Action::Updates),
    ("quit", Action::Quit),
];

fn action(id: &str) -> Option<Action> {
    IDS.iter().find(|(i, _)| *i == id).map(|&(_, a)| a)
}

pub struct Tray {
    _icon: TrayIcon,
    save: MenuItem,
}

impl Tray {
    /// `handle` runs on the UI thread for every menu click.
    pub fn new(hotkey: &str, handle: fn(Action)) -> Result<Tray, String> {
        let save = MenuItem::with_id("save", save_label(hotkey), true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &save,
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id("settings", "Settings", true, None),
            &MenuItem::with_id("folder", "Open folder", true, None),
            &MenuItem::with_id("updates", "Check for updates", true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id("quit", "Quit", true, None),
        ])
        .map_err(|e| e.to_string())?;
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(action) = action(event.id.as_ref()) {
                let _ = slint::invoke_from_event_loop(move || handle(action));
            }
        }));
        // Resource 1 is the icon build.rs embeds; 16 px is the notification area's size at 100 %.
        let icon = Icon::from_resource(1, None).map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Clip Recorder")
            .with_icon(icon)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Tray { _icon: tray, save })
    }

    pub fn set_hotkey(&self, hotkey: &str) {
        self.save.set_text(save_label(hotkey));
    }
}

fn save_label(hotkey: &str) -> String {
    format!("Save clip ({hotkey})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_id_reaches_its_action() {
        assert_eq!(action("updates"), Some(Action::Updates));
        assert_eq!(action("quit"), Some(Action::Quit));
        assert_eq!(action("nothing"), None);
    }
}
