//! What the Settings window shows and what its Save does, without the window. Pickers work on
//! indices, so no label is ever parsed back (the Python app's "N: name" and "(Auto" contracts).

use crate::config::{Config, FPS_OPTIONS};

/// Index 0 of each device picker: empty in config.json.
pub const AUTO: &str = "Auto (system default)";

/// What a Save has to restart. Audio restarts only for a device change: restarting it for
/// monitor/fps/buffer once re-exposed a flaky loopback reopen and desynced the clip.
#[derive(Debug, PartialEq, Default)]
pub struct Changes {
    pub audio: bool,
    pub video: bool,
    pub hotkey: bool,
}

pub fn changes(old: &Config, new: &Config) -> Changes {
    Changes {
        audio: old.loopback_device != new.loopback_device || old.mic_device != new.mic_device,
        video: old.monitor != new.monitor || old.fps != new.fps || old.buffer_seconds != new.buffer_seconds,
        hotkey: old.hotkey != new.hotkey,
    }
}

/// The picker's entries, Auto first, and the one to pre-select: what the config says, never
/// the device Auto detected (a Save would pin it). A configured device that is unplugged stays
/// listed, so a Save made for something else does not quietly switch it to Auto.
pub fn device_choices(mut live: Vec<String>, configured: &str) -> (Vec<String>, usize) {
    if !configured.is_empty() && !live.iter().any(|d| d == configured) {
        live.push(configured.into());
    }
    let index = live.iter().position(|d| d == configured).map_or(0, |i| i + 1);
    (std::iter::once(AUTO.to_string()).chain(live).collect(), index)
}

/// The config value for a picked entry.
pub fn device_at(choices: &[String], index: usize) -> String {
    if index == 0 { String::new() } else { choices.get(index).cloned().unwrap_or_default() }
}

/// The Python app stored loopback names with " [Loopback]"; config files may still carry it.
pub fn configured_device(name: &str) -> &str {
    name.trim_end_matches(" [Loopback]")
}

/// Which listed device a configured name means: the exact name first, then one that contains
/// it. Substring first picked "USB Headphones" for "Headphones" (fixed once in Python already).
pub fn pick_device(names: &[String], configured: &str) -> Option<usize> {
    if configured.is_empty() {
        return None;
    }
    names.iter().position(|n| n == configured).or_else(|| names.iter().position(|n| n.contains(configured)))
}

/// Above 60 fps only DXGI capture keeps up.
pub fn fps_choices(ddagrab: bool) -> Vec<u32> {
    FPS_OPTIONS.into_iter().filter(|&f| ddagrab || f <= 60).collect()
}

/// Index in `options` of `value`, else the last one (an fps the hardware cannot do).
pub fn index_of(options: &[u32], value: u32) -> usize {
    options.iter().position(|&o| o == value).unwrap_or(options.len().saturating_sub(1))
}

/// Explorer's StartupApproved entry: absent is enabled; an odd first byte is "disabled in Task
/// Manager" (or by us).
pub fn startup_approved(entry: Option<&[u8]>) -> bool {
    entry.and_then(|b| b.first()).is_none_or(|b| b & 1 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_device_change_restarts_the_audio() {
        let old = Config::default();
        let video = Config { fps: 120, buffer_seconds: 90, monitor: 1, ..old.clone() };
        assert_eq!(changes(&old, &video), Changes { video: true, ..Default::default() });
        let mic = Config { mic_device: "Microphone (USB)".into(), ..old.clone() };
        assert_eq!(changes(&old, &mic), Changes { audio: true, ..Default::default() });
        let key = Config { hotkey: "Ctrl+F9".into(), output_folder: "D:\\Clips".into(), ..old.clone() };
        assert_eq!(changes(&old, &key), Changes { hotkey: true, ..Default::default() });
    }

    fn live() -> Vec<String> {
        vec!["Speakers (Realtek)".into(), "Headphones (USB)".into()]
    }

    #[test]
    fn auto_in_config_preselects_auto_and_saves_back_empty() {
        let (choices, index) = device_choices(live(), "");
        assert_eq!((choices[0].as_str(), index), (AUTO, 0));
        assert_eq!(device_at(&choices, index), "");
    }

    #[test]
    fn a_pinned_device_is_preselected_and_saved_back_unchanged() {
        let (choices, index) = device_choices(live(), "Headphones (USB)");
        assert_eq!((choices.len(), index), (3, 2));
        assert_eq!(device_at(&choices, index), "Headphones (USB)");
    }

    #[test]
    fn an_unplugged_pinned_device_stays_pinned() {
        let (choices, index) = device_choices(live(), "Bluetooth (Off)");
        assert_eq!(device_at(&choices, index), "Bluetooth (Off)");
    }

    #[test]
    fn a_python_era_loopback_name_still_finds_its_device() {
        assert_eq!(configured_device("Haut-parleurs (Realtek(R) Audio) [Loopback]"), "Haut-parleurs (Realtek(R) Audio)");
        assert_eq!(configured_device("Microphone (USB)"), "Microphone (USB)");
    }

    #[test]
    fn the_exact_device_name_wins_over_a_longer_one_listed_first() {
        let names: Vec<String> = vec!["USB Headphones (Realtek)".into(), "Headphones (Realtek)".into()];
        assert_eq!(pick_device(&names, "Headphones (Realtek)"), Some(1));
        assert_eq!(pick_device(&names, "USB Headphones"), Some(0), "a partial name still finds its device");
        assert_eq!(pick_device(&names, "Speakers"), None);
        assert_eq!(pick_device(&names, ""), None, "empty is Auto");
    }

    #[test]
    fn above_60_fps_needs_ddagrab() {
        assert_eq!(fps_choices(true), [30, 60, 120, 240]);
        assert_eq!(fps_choices(false), [30, 60]);
        assert_eq!(index_of(&fps_choices(false), 240), 1, "falls back to the highest offered");
        assert_eq!(index_of(&fps_choices(true), 120), 2);
    }

    #[test]
    fn startup_approved_reads_like_task_manager() {
        assert!(startup_approved(None));
        assert!(startup_approved(Some(&[2, 0, 0])));
        assert!(!startup_approved(Some(&[3, 0, 0])));
        assert!(startup_approved(Some(&[])));
    }
}
