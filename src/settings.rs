//! What the Settings window shows and what its Save does, without the window. Pickers work on
//! indices, so no label is ever parsed back (the Python app's "N: name" and "(Auto" contracts).

use crate::config::{Config, FPS_OPTIONS, MonitorChoice};

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
    // The name the capture uses, so a Python-era " [Loopback]" name is not a phantom device.
    let configured = configured_device(configured);
    if !configured.is_empty() && !live.iter().any(|d| d == configured) {
        live.push(configured.into());
    }
    let index = live.iter().position(|d| d == configured).map_or(0, |i| i + 1);
    (std::iter::once(AUTO.to_string()).chain(live).collect(), index)
}

/// The config value for a picked entry.
pub fn device_at(choices: &[String], index: usize) -> String {
    if index == 0 {
        String::new()
    } else {
        choices.get(index).cloned().unwrap_or_default()
    }
}

/// The hotkey a Save leaves in config.json: the new one only if it registered. Writing an
/// unconfirmed one pinned a string no registration backed, and since Settings compares strings,
/// re-picking it could never retry.
pub fn hotkey_after_save<'a>(old: &'a str, new: &'a str, registered: bool) -> &'a str {
    if registered { new } else { old }
}

/// What a Save does to "Start with Windows": Some(on) to write it, None to leave it. Ticked, it is
/// written every time (an older install's path would otherwise start nothing); unticked, only
/// switched off if it is on. A dev build offers no switch at all.
pub fn startup_change(available: bool, ticked: bool, enabled: bool) -> Option<bool> {
    (available && (ticked || enabled)).then_some(ticked)
}

/// The 12-byte StartupApproved entry Task Manager writes: 02 on, 03 off with the time it was
/// switched off (a FILETIME at offset 4).
pub fn approved_entry(on: bool, filetime: u64) -> [u8; 12] {
    let mut entry = [0u8; 12];
    if on {
        entry[0] = 2;
    } else {
        entry[0] = 3;
        entry[4..].copy_from_slice(&filetime.to_le_bytes());
    }
    entry
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
    names
        .iter()
        .position(|n| n == configured)
        .or_else(|| names.iter().position(|n| n.contains(configured)))
}

/// The screen the capture, the banner and Settings use, among the device names listed primary
/// first: the chosen one if it is there, else the primary; an older index is clamped.
pub fn screen_index<'a>(choice: &MonitorChoice, names: impl IntoIterator<Item = &'a str>) -> usize {
    let names: Vec<&str> = names.into_iter().collect();
    match choice {
        MonitorChoice::Device(name) => names.iter().position(|n| n == name).unwrap_or(0),
        MonitorChoice::Index(i) => (*i).min(names.len().saturating_sub(1)),
    }
}

/// Where the capture must move, if the screen `choice` picks now is not the one it runs on: back to
/// the chosen screen when it returns, to the primary when it goes. None with no screen listed.
pub fn screen_to_move_to<'a>(
    choice: &MonitorChoice,
    names: impl IntoIterator<Item = &'a str>,
    capturing_on: &str,
) -> Option<usize> {
    let names: Vec<&str> = names.into_iter().collect();
    let wanted = screen_index(choice, names.iter().copied());
    names.get(wanted).filter(|&&n| n != capturing_on).map(|_| wanted)
}

/// Health ticks a screen change must hold before the capture follows it (10 s).
pub const MOVE_TICKS: u32 = 5;

/// Waits out short screen changes: a DisplayPort screen can leave the list while it sleeps or
/// switches input, and each move wipes the replay buffer. A capture whose ffmpeg dies restarts
/// on the screen picked then, without waiting.
#[derive(Debug, Default)]
pub struct ScreenMove {
    pending: Option<(String, u32)>,
}

impl ScreenMove {
    /// One tick with the screen the capture should move to, if any (screen_to_move_to): that
    /// screen once it has been the answer MOVE_TICKS ticks in a row.
    pub fn tick(&mut self, wanted: Option<&str>) -> Option<String> {
        let Some(wanted) = wanted else {
            self.pending = None;
            return None;
        };
        let held = match &self.pending {
            Some((name, n)) if name == wanted => n + 1,
            _ => 1,
        };
        if held >= MOVE_TICKS {
            self.pending = None;
            return Some(wanted.to_string());
        }
        self.pending = Some((wanted.to_string(), held));
        None
    }
}

/// Above 60 fps only DXGI capture keeps up.
pub fn fps_choices(ddagrab: bool) -> Vec<u32> {
    FPS_OPTIONS.into_iter().filter(|&f| ddagrab || f <= 60).collect()
}

/// Index in `options` of `value`, else the last one (an fps the hardware cannot do).
pub fn index_of(options: &[u32], value: u32) -> usize {
    options
        .iter()
        .position(|&o| o == value)
        .unwrap_or(options.len().saturating_sub(1))
}

/// Explorer's StartupApproved entry: absent is enabled; an odd first byte is "disabled in Task
/// Manager" (or by us).
pub fn startup_approved(entry: Option<&[u8]>) -> bool {
    entry.and_then(|b| b.first()).is_none_or(|b| b & 1 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each field on its own: every one of them restarts what it should, and only that.
    #[test]
    fn each_setting_restarts_only_its_own_part() {
        let old = Config::default();
        let audio = Changes {
            audio: true,
            ..Default::default()
        };
        let video = Changes {
            video: true,
            ..Default::default()
        };
        let cases = [
            (
                Config {
                    loopback_device: "Speakers".into(),
                    ..old.clone()
                },
                audio,
            ),
            (
                Config {
                    mic_device: "Mic".into(),
                    ..old.clone()
                },
                Changes {
                    audio: true,
                    ..Default::default()
                },
            ),
            (
                Config {
                    monitor: MonitorChoice::Index(1),
                    ..old.clone()
                },
                video,
            ),
            (
                Config {
                    fps: 120,
                    ..old.clone()
                },
                Changes {
                    video: true,
                    ..Default::default()
                },
            ),
            (
                Config {
                    buffer_seconds: 90,
                    ..old.clone()
                },
                Changes {
                    video: true,
                    ..Default::default()
                },
            ),
            (
                Config {
                    output_folder: "D:\\Clips".into(),
                    ..old.clone()
                },
                Changes::default(),
            ),
        ];
        for (new, expected) in cases {
            assert_eq!(changes(&old, &new), expected, "{new:?}");
        }
    }

    #[test]
    fn only_a_device_change_restarts_the_audio() {
        let old = Config::default();
        let video = Config {
            fps: 120,
            buffer_seconds: 90,
            monitor: MonitorChoice::Index(1),
            ..old.clone()
        };
        assert_eq!(
            changes(&old, &video),
            Changes {
                video: true,
                ..Default::default()
            }
        );
        let mic = Config {
            mic_device: "Microphone (USB)".into(),
            ..old.clone()
        };
        assert_eq!(
            changes(&old, &mic),
            Changes {
                audio: true,
                ..Default::default()
            }
        );
        let key = Config {
            hotkey: "Ctrl+F9".into(),
            output_folder: "D:\\Clips".into(),
            ..old.clone()
        };
        assert_eq!(
            changes(&old, &key),
            Changes {
                hotkey: true,
                ..Default::default()
            }
        );
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
    fn a_hotkey_that_did_not_register_is_never_saved() {
        assert_eq!(hotkey_after_save("Ctrl+Alt+R", "Ctrl+F9", true), "Ctrl+F9");
        assert_eq!(hotkey_after_save("Ctrl+Alt+R", "Ctrl+F9", false), "Ctrl+Alt+R");
    }

    #[test]
    fn start_with_windows_is_rewritten_when_ticked_and_switched_off_only_when_on() {
        assert_eq!(
            startup_change(true, true, true),
            Some(true),
            "ticked: re-pointed every time"
        );
        assert_eq!(startup_change(true, true, false), Some(true));
        assert_eq!(startup_change(true, false, true), Some(false));
        assert_eq!(startup_change(true, false, false), None);
        assert_eq!(startup_change(false, true, true), None, "dev build");
    }

    #[test]
    fn the_approved_entry_reads_back_as_written() {
        assert!(startup_approved(Some(&approved_entry(true, 0))));
        let off = approved_entry(false, 0x0102_0304_0506_0708);
        assert!(!startup_approved(Some(&off)));
        assert_eq!(off[4..], 0x0102_0304_0506_0708u64.to_le_bytes());
    }

    #[test]
    fn a_python_era_loopback_name_is_not_listed_twice() {
        let (choices, index) = device_choices(live(), "Speakers (Realtek) [Loopback]");
        assert_eq!(choices.len(), 3, "{choices:?}");
        assert_eq!(
            device_at(&choices, index),
            "Speakers (Realtek)",
            "saved back without the suffix"
        );
    }

    #[test]
    fn a_python_era_loopback_name_still_finds_its_device() {
        assert_eq!(
            configured_device("Haut-parleurs (Realtek(R) Audio) [Loopback]"),
            "Haut-parleurs (Realtek(R) Audio)"
        );
        assert_eq!(configured_device("Microphone (USB)"), "Microphone (USB)");
    }

    #[test]
    fn the_exact_device_name_wins_over_a_longer_one_listed_first() {
        let names: Vec<String> = vec!["USB Headphones (Realtek)".into(), "Headphones (Realtek)".into()];
        assert_eq!(pick_device(&names, "Headphones (Realtek)"), Some(1));
        assert_eq!(
            pick_device(&names, "USB Headphones"),
            Some(0),
            "a partial name still finds its device"
        );
        assert_eq!(pick_device(&names, "Speakers"), None);
        assert_eq!(pick_device(&names, ""), None, "empty is Auto");
    }

    #[test]
    fn above_60_fps_needs_ddagrab() {
        assert_eq!(fps_choices(true), [30, 60, 120, 240]);
        assert_eq!(fps_choices(false), [30, 60]);
        assert_eq!(
            index_of(&fps_choices(false), 240),
            1,
            "falls back to the highest offered"
        );
        assert_eq!(index_of(&fps_choices(true), 120), 2);
    }

    const NAMES: [&str; 3] = [r"\\.\DISPLAY1", r"\\.\DISPLAY3", r"\\.\DISPLAY2"];

    /// By name: unplugging another screen no longer shifts which one is captured.
    #[test]
    fn the_chosen_screen_is_found_by_its_name() {
        let second = MonitorChoice::Device(r"\\.\DISPLAY2".into());
        assert_eq!(screen_index(&second, NAMES), 2);
        assert_eq!(screen_index(&second, [NAMES[0], NAMES[2]]), 1, "DISPLAY3 unplugged");
        assert_eq!(
            screen_index(&second, [NAMES[0], NAMES[1]]),
            0,
            "gone itself: the primary"
        );
    }

    /// A DisplayPort screen can leave the list while it sleeps: each move wipes the buffer, so a
    /// change has to hold MOVE_TICKS (10 s) before the capture follows it.
    #[test]
    fn a_screen_change_must_hold_before_the_capture_follows() {
        assert_eq!(MOVE_TICKS, 5, "10 s at one tick every 2 s");
        let mut m = ScreenMove::default();
        for _ in 1..MOVE_TICKS {
            assert_eq!(m.tick(Some("B")), None);
        }
        assert_eq!(m.tick(Some("B")), Some("B".to_string()), "held long enough");
        // A blip: the change goes away before it held, and the count starts over.
        for _ in 1..MOVE_TICKS {
            assert_eq!(m.tick(Some("A")), None);
        }
        assert_eq!(m.tick(None), None);
        assert_eq!(m.tick(Some("A")), None, "counting again from one");
        // Another target restarts the count too.
        for _ in 2..MOVE_TICKS {
            assert_eq!(m.tick(Some("A")), None);
        }
        assert_eq!(m.tick(Some("C")), None);
    }

    #[test]
    fn an_older_index_is_clamped_to_the_screens_there_are() {
        assert_eq!(screen_index(&MonitorChoice::Index(1), NAMES), 1);
        assert_eq!(screen_index(&MonitorChoice::Index(5), NAMES), 2, "the last one");
        assert_eq!(
            screen_index(&MonitorChoice::Index(0), [] as [&str; 0]),
            0,
            "none listed"
        );
    }

    /// The capture moves when the screen the config picks now is another one: back to the chosen
    /// screen when it returns, to the primary when it goes. Same screen: never (a mode change of
    /// the screen must not cost the buffer).
    #[test]
    fn the_capture_moves_only_when_another_screen_is_picked() {
        let second = MonitorChoice::Device(r"\\.\DISPLAY2".into());
        assert_eq!(
            screen_to_move_to(&second, NAMES, r"\\.\DISPLAY1"),
            Some(2),
            "it came back"
        );
        assert_eq!(screen_to_move_to(&second, NAMES, r"\\.\DISPLAY2"), None);
        assert_eq!(
            screen_to_move_to(&second, [NAMES[0]], r"\\.\DISPLAY2"),
            Some(0),
            "it went: the primary"
        );
        assert_eq!(
            screen_to_move_to(&second, [] as [&str; 0], r"\\.\DISPLAY2"),
            None,
            "no screen: stay"
        );
    }

    #[test]
    fn startup_approved_reads_like_task_manager() {
        assert!(startup_approved(None));
        assert!(startup_approved(Some(&[2, 0, 0])));
        assert!(!startup_approved(Some(&[3, 0, 0])));
        assert!(startup_approved(Some(&[])));
    }
}
