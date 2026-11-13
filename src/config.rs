//! config.json: the same keys as the Python app. It is hand-editable, so it is parsed leniently
//! once, here, and every value the rest of the app sees is valid.

use serde::Serialize;
use std::path::Path;

pub const FPS_OPTIONS: [u32; 4] = [30, 60, 120, 240];
pub const BUFFER_OPTIONS: [u32; 5] = [15, 30, 60, 90, 120];
pub const DEFAULT_HOTKEY: &str = "Ctrl+Alt+R";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Config {
    /// Index into the primary-first monitor list.
    pub monitor: usize,
    /// One of FPS_OPTIONS.
    pub fps: u32,
    /// One of BUFFER_OPTIONS.
    pub buffer_seconds: u32,
    /// Empty: the Videos known folder's ClipRecorder.
    pub output_folder: String,
    /// Empty: Auto (the Windows default output).
    pub loopback_device: String,
    /// Empty: Auto (the Windows default input).
    pub mic_device: String,
    pub hotkey: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            monitor: 0,
            fps: 60,
            buffer_seconds: 30,
            output_folder: String::new(),
            loopback_device: String::new(),
            mic_device: String::new(),
            hotkey: DEFAULT_HOTKEY.into(),
        }
    }
}

impl Config {
    /// Any text in, a valid Config out: a missing, broken or hand-edited file never stops the app.
    pub fn parse(json: &str) -> Config {
        let d = Config::default();
        let Ok(serde_json::Value::Object(v)) = serde_json::from_str(json) else {
            return d;
        };
        let number = |key: &str| match v.get(key) {
            Some(serde_json::Value::Number(n)) => n.as_u64(),
            Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        };
        let text = |key: &str| v.get(key).and_then(|s| s.as_str()).map(str::to_owned);
        let pick = |key: &str, options: &[u32], default: u32| {
            number(key)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| options.contains(n))
                .unwrap_or(default)
        };
        Config {
            monitor: number("monitor").map_or(d.monitor, |n| n as usize),
            fps: pick("fps", &FPS_OPTIONS, d.fps),
            buffer_seconds: pick("buffer_seconds", &BUFFER_OPTIONS, d.buffer_seconds),
            output_folder: text("output_folder").unwrap_or(d.output_folder),
            loopback_device: text("loopback_device").unwrap_or(d.loopback_device),
            mic_device: text("mic_device").unwrap_or(d.mic_device),
            hotkey: text("hotkey").filter(|h| !h.trim().is_empty()).unwrap_or(d.hotkey),
        }
    }

    /// Missing or unreadable file: the defaults.
    pub fn load(path: &Path) -> Config {
        std::fs::read_to_string(path)
            .map(|s| Config::parse(&s))
            .unwrap_or_default()
    }

    /// Written aside then renamed over, so a crash mid-write never leaves half a config.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_or_missing_input_gives_the_defaults() {
        for text in ["", "not json", "[1, 2]", "null"] {
            assert_eq!(Config::parse(text), Config::default(), "input {text:?}");
        }
        assert_eq!(Config::load(Path::new(r"C:\no\such\config.json")), Config::default());
    }

    #[test]
    fn missing_keys_take_defaults_and_present_ones_are_kept() {
        let c = Config::parse(r#"{"fps": 240, "hotkey": "Ctrl+Shift+F9", "mic_device": "Microphone (USB)"}"#);
        assert_eq!(
            (c.fps, c.hotkey.as_str(), c.mic_device.as_str()),
            (240, "Ctrl+Shift+F9", "Microphone (USB)")
        );
        assert_eq!((c.buffer_seconds, c.monitor), (30, 0));
    }

    #[test]
    fn hand_edited_numbers_are_parsed_leniently() {
        // "fps": "120" used to crash the Python app's >= comparison and leave it capturing nothing.
        let c = Config::parse(r#"{"fps": "120", "buffer_seconds": " 90 ", "monitor": "1"}"#);
        assert_eq!((c.fps, c.buffer_seconds, c.monitor), (120, 90, 1));
    }

    #[test]
    fn values_outside_the_options_fall_back_to_the_defaults() {
        let c = Config::parse(r#"{"fps": 144, "buffer_seconds": 0, "hotkey": ""}"#);
        assert_eq!((c.fps, c.buffer_seconds, c.hotkey.as_str()), (60, 30, DEFAULT_HOTKEY));
    }

    #[test]
    fn a_saved_config_loads_back_identical() {
        let dir = std::env::temp_dir().join(format!("crtest_config_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.json");
        let c = Config {
            fps: 240,
            buffer_seconds: 90,
            loopback_device: "Haut-parleurs (Realtek(R) Audio)".into(),
            ..Config::default()
        };
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path), c);
        assert!(
            !dir.join("config.json.tmp").exists(),
            "the temporary file was left behind"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
