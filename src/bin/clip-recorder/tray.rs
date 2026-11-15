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

fn id(action: Action) -> &'static str {
    IDS.iter().find(|(_, a)| *a == action).map_or("", |&(i, _)| i)
}

pub struct Tray {
    _icon: TrayIcon,
    save: MenuItem,
}

impl Tray {
    /// `handle` runs on the UI thread for every menu click.
    pub fn new(hotkey: &str, handle: fn(Action)) -> Result<Tray, String> {
        let item = |a: Action, text: &str| MenuItem::with_id(id(a), text, true, None);
        let save = item(Action::Save, &save_label(hotkey));
        let menu = Menu::new();
        menu.append_items(&[
            &save,
            &PredefinedMenuItem::separator(),
            &item(Action::Settings, "Settings"),
            &item(Action::OpenFolder, "Open folder"),
            &item(Action::Updates, "Check for updates"),
            &PredefinedMenuItem::separator(),
            &item(Action::Quit, "Quit"),
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

    /// The menu items take their ids from id(), and a click is routed by action(): every action
    /// must survive the round trip, or its item does nothing.
    #[test]
    fn every_menu_id_reaches_its_action() {
        for a in [
            Action::Save,
            Action::Settings,
            Action::OpenFolder,
            Action::Updates,
            Action::Quit,
        ] {
            assert_eq!(action(id(a)), Some(a));
        }
        assert_eq!(action("nothing"), None);
    }
}
