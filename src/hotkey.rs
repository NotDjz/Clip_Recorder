//! The save hotkey, written "Ctrl+Alt+R" in config.json, as RegisterHotKey wants it.

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
/// Holding the keys saves once, not once per auto-repeat.
pub const MOD_NOREPEAT: u32 = 0x4000;

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

    #[test]
    fn a_key_without_a_modifier_or_garbage_falls_back_to_the_default() {
        for text in ["R", "Ctrl+Alt", "Ctrl+Alt+Escape", "", "Win+R"] {
            assert_eq!(parse(text), DEFAULT, "{text:?}");
        }
    }
}
