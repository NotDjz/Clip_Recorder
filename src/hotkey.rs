//! The save hotkey, written "Ctrl+Alt+R" in config.json, as RegisterHotKey wants it.

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
/// Holding the keys saves once, not once per auto-repeat.
pub const MOD_NOREPEAT: u32 = 0x4000;

/// What the keyboard hook does with a key event.
#[derive(Debug, PartialEq)]
pub enum Key {
    Pass,
    /// Save, and eat the key: an eaten press never also arrives as WM_HOTKEY.
    Press,
    /// An auto-repeat of a press: eaten, no save.
    Eat,
}

/// The hotkey as a keyboard hook sees it. Some games (League of Legends) turn registered hotkeys
/// off while they have the foreground; a low-level hook still sees the keys.
pub struct Watch {
    hotkey: Hotkey,
    /// When the hotkey's key last went down (the event's ms tick), while it is held.
    last_down: Option<u32>,
}

impl Watch {
    pub fn new(hotkey: Hotkey) -> Watch {
        Watch {
            hotkey,
            last_down: None,
        }
    }

    /// `mods`: the MOD_ keys held, read only for the hotkey's key (the hook sees every key typed).
    /// A press needs exactly the hotkey's modifiers, as RegisterHotKey does; so does a repeat,
    /// or a plain key (the ultimate in League) right after a save would be eaten.
    pub fn key(&mut self, vk: u32, down: bool, mods: impl FnOnce() -> u32, time_ms: u32) -> Key {
        // Longer than any keyboard repeat delay (1 s at most): a key-up no hook saw (the UAC
        // secure desktop) must not turn the next real press into an "auto-repeat".
        const REPEAT_MS: u32 = 1500;
        if vk != self.hotkey.vk {
            return Key::Pass;
        }
        let repeat = self.last_down.is_some_and(|t| time_ms.wrapping_sub(t) < REPEAT_MS);
        self.last_down = None;
        if !down || mods() != self.hotkey.modifiers & !MOD_NOREPEAT {
            return Key::Pass;
        }
        self.last_down = Some(time_ms);
        if repeat { Key::Eat } else { Key::Press }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

/// Ctrl+Alt+R: the default, and the answer to anything unusable (no key, or no modifier).
pub const DEFAULT: Hotkey = Hotkey {
    modifiers: MOD_NOREPEAT | MOD_CONTROL | MOD_ALT,
    vk: 0x52,
};

/// Keys: A-Z, 0-9, F1-F12. Modifiers: Ctrl, Alt, Shift. Case and spaces don't matter.
pub fn parse(text: &str) -> Hotkey {
    let mut modifiers = MOD_NOREPEAT;
    let mut vk = None;
    for part in text.split('+').map(|p| p.trim().to_ascii_uppercase()) {
        match part.as_str() {
            "CTRL" => modifiers |= MOD_CONTROL,
            "ALT" => modifiers |= MOD_ALT,
            "SHIFT" => modifiers |= MOD_SHIFT,
            key => vk = key_code(key).or(vk),
        }
    }
    match vk {
        Some(vk) if modifiers != MOD_NOREPEAT => Hotkey { modifiers, vk },
        _ => DEFAULT,
    }
}

/// The combination the Settings capture writes to config.json, or None when it would not
/// register as typed (no modifier, or a key RegisterHotKey is not given).
pub fn format(ctrl: bool, alt: bool, shift: bool, key: &str) -> Option<String> {
    let key = key.to_ascii_uppercase();
    key_code(&key)?;
    let mods: Vec<&str> = [(ctrl, "Ctrl"), (alt, "Alt"), (shift, "Shift")]
        .into_iter()
        .filter(|m| m.0)
        .map(|m| m.1)
        .collect();
    (!mods.is_empty()).then(|| format!("{}+{key}", mods.join("+")))
}

fn key_code(key: &str) -> Option<u32> {
    let bytes = key.as_bytes();
    match bytes {
        [c] if c.is_ascii_uppercase() || c.is_ascii_digit() => Some(*c as u32),
        [b'F', rest @ ..] => match std::str::from_utf8(rest).ok()?.parse::<u32>().ok()? {
            n @ 1..=12 => Some(0x6F + n),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_parses_to_ctrl_alt_r() {
        assert_eq!(parse("Ctrl+Alt+R"), DEFAULT);
    }

    #[test]
    fn letters_digits_and_function_keys() {
        assert_eq!(
            parse("Ctrl+Shift+F9"),
            Hotkey {
                modifiers: MOD_NOREPEAT | MOD_CONTROL | MOD_SHIFT,
                vk: 0x78
            }
        );
        assert_eq!(
            parse(" alt + 5 "),
            Hotkey {
                modifiers: MOD_NOREPEAT | MOD_ALT,
                vk: 0x35
            }
        );
        assert_eq!(parse("Ctrl+F12").vk, 0x7B);
    }

    #[test]
    fn a_captured_combination_parses_back_to_itself() {
        let text = format(true, false, true, "f9").unwrap();
        assert_eq!(text, "Ctrl+Shift+F9");
        assert_eq!(
            parse(&text),
            Hotkey {
                modifiers: MOD_NOREPEAT | MOD_CONTROL | MOD_SHIFT,
                vk: 0x78
            }
        );
        assert_eq!(format(true, true, false, "r").as_deref(), Some("Ctrl+Alt+R"));
    }

    #[test]
    fn a_capture_without_a_modifier_or_with_an_odd_key_is_refused() {
        assert_eq!(format(false, false, false, "R"), None);
        assert_eq!(format(true, false, false, "Escape"), None);
        assert_eq!(format(true, false, false, ""), None);
    }

    const DOWN: bool = true;
    const UP: bool = false;
    const CTRL_ALT: u32 = MOD_CONTROL | MOD_ALT;
    const R: u32 = 0x52;

    #[test]
    fn the_hook_saves_once_on_the_exact_combination_and_eats_its_repeats() {
        let mut watch = Watch::new(DEFAULT);
        assert_eq!(watch.key(R, DOWN, || CTRL_ALT, 1000), Key::Press);
        assert_eq!(watch.key(R, DOWN, || CTRL_ALT, 1500), Key::Eat, "auto-repeat");
        assert_eq!(watch.key(R, DOWN, || CTRL_ALT, 1530), Key::Eat, "auto-repeat");
        assert_eq!(watch.key(R, UP, || CTRL_ALT, 1600), Key::Pass);
        assert_eq!(
            watch.key(R, DOWN, || CTRL_ALT, 1700),
            Key::Press,
            "a new press after the key-up"
        );
    }

    #[test]
    fn the_hook_lets_every_other_key_through() {
        let mut watch = Watch::new(DEFAULT);
        assert_eq!(watch.key(R, DOWN, || MOD_CONTROL, 0), Key::Pass, "a modifier missing");
        assert_eq!(
            watch.key(R, DOWN, || CTRL_ALT | MOD_SHIFT, 0),
            Key::Pass,
            "an extra modifier"
        );
        assert_eq!(watch.key(R, DOWN, || CTRL_ALT | MOD_WIN, 0), Key::Pass, "Win held too");
        assert_eq!(watch.key(R, DOWN, || 0, 0), Key::Pass, "R alone (a spell in game)");
        assert_eq!(watch.key(0x54, DOWN, || CTRL_ALT, 0), Key::Pass, "another key");
        assert_eq!(
            watch.key(R, DOWN, || CTRL_ALT, 0),
            Key::Press,
            "none of those counted as held"
        );
    }

    #[test]
    fn a_key_up_the_hook_never_saw_costs_no_press() {
        // The key-up can happen where no hook sees it (the secure desktop of a UAC prompt):
        // a press long after the last one is a new press, not an auto-repeat.
        let mut watch = Watch::new(DEFAULT);
        assert_eq!(watch.key(R, DOWN, || CTRL_ALT, u32::MAX - 100), Key::Press);
        assert_eq!(
            watch.key(R, DOWN, || CTRL_ALT, 5000),
            Key::Press,
            "5 s later, across the tick wrap"
        );
        assert_eq!(
            watch.key(R, DOWN, || CTRL_ALT, 6499),
            Key::Eat,
            "under 1.5 s: an auto-repeat"
        );
        assert_eq!(
            watch.key(R, DOWN, || CTRL_ALT, 7999),
            Key::Press,
            "1.5 s after the last one"
        );
    }

    #[test]
    fn a_plain_key_right_after_a_save_is_never_eaten() {
        // In game R is the ultimate: a save's key-up the hook missed, or Ctrl+Alt let go with R
        // still held, must not swallow it.
        let mut watch = Watch::new(DEFAULT);
        assert_eq!(watch.key(R, DOWN, || CTRL_ALT, 1000), Key::Press);
        assert_eq!(watch.key(R, DOWN, || 0, 1300), Key::Pass, "R alone 0.3 s later");
        assert_eq!(watch.key(R, DOWN, || 0, 1330), Key::Pass, "and its own repeats");
        assert_eq!(
            watch.key(R, DOWN, || CTRL_ALT, 1400),
            Key::Press,
            "a real press after it"
        );
    }

    #[test]
    fn the_modifiers_are_read_only_for_the_hotkey_key() {
        let mut watch = Watch::new(DEFAULT);
        let unread = || -> u32 { panic!("read for another key") };
        assert_eq!(watch.key(0x57, DOWN, unread, 0), Key::Pass, "W while moving");
    }

    #[test]
    fn a_key_without_a_modifier_or_garbage_falls_back_to_the_default() {
        for text in ["R", "Ctrl+Alt", "Ctrl+Alt+Escape", "", "Win+R"] {
            assert_eq!(parse(text), DEFAULT, "{text:?}");
        }
    }
}
