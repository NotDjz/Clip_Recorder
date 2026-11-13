//! Guards on files no test can run: the installer, the build script, and code whose rule is
//! about where something happens (the real-time audio callback, the order of startup steps).
//! Each check reads code with its comments removed, so a rule mentioned in a comment can never
//! satisfy it.

fn read(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

/// Rust (or PowerShell, `marker` = "#") source without its line comments.
fn code(path: &str, marker: &str) -> String {
    read(path)
        .lines()
        .filter(|l| !l.trim_start().starts_with(marker) && !l.trim_start().starts_with("///"))
        .map(|l| l.split(" // ").next().unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// XML without its comments.
fn xml(path: &str) -> String {
    let text = read(path);
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = rest[start..].find("-->").map_or("", |end| &rest[start + end + 3..]);
    }
    out + rest
}

/// The text between `from` and the first `to` after it.
fn between<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
    let start = text.find(from).unwrap_or_else(|| panic!("{from:?} not found"));
    let len = text[start..].find(to).unwrap_or_else(|| panic!("{to:?} not found after {from:?}"));
    &text[start..start + len]
}

fn position(text: &str, needle: &str) -> usize {
    text.find(needle).unwrap_or_else(|| panic!("{needle:?} not found"))
}

/// The installing user starts with Windows through HKCU (HKLM's on/off needs admin), and the
/// MSI never sends WM_CLOSE to winit's hidden windows (a windowless process blocked upgrades).
#[test]
fn the_msi_writes_the_run_value_in_hkcu_and_never_closes_the_app_itself() {
    let wxs = xml("installer/main.wxs");
    let run = between(&wxs, "<RegistryValue", "/>");
    assert!(run.contains(r#"Root="HKCU""#) && run.contains(r"CurrentVersion\Run"), "{run}");
    assert!(!wxs.contains("CloseApplication"), "util:CloseApplication blocks upgrades");
    assert!(wxs.contains("<MajorUpgrade"), "an upgrade must replace the old install");
}

/// The shipped ffmpeg is the pinned one: checked before it is moved into place, an unverified
/// one deleted first, and the MSI never built after a failed cargo build (it once shipped the
/// previous exe).
#[test]
fn the_build_ships_only_a_verified_ffmpeg_and_a_fresh_exe() {
    let ps = code("build-msi.ps1", "#");
    assert!(read("build-msi.ps1").starts_with("#Requires -Version 7"), "PowerShell 5 stops before building");
    let check = position(&ps, "if (-not (Test-Pinned $part))");
    assert!(check < position(&ps, "Move-Item $part $ffmpeg"), "moved before its hash was checked");
    assert!(position(&ps, "Remove-Item $ffmpeg") < position(&ps, "Invoke-WebRequest"), "a stale exe stays where the build looks");
    let built = position(&ps, "cargo build --release --locked");
    let stop = position(&ps, "if ($LASTEXITCODE) { exit $LASTEXITCODE }");
    assert!(built < stop && stop < position(&ps, "wix build"), "wix runs after a failed cargo build");
}

/// Real-time code: no logging and no file access in the audio data callback, or WASAPI
/// delivery stalls and the gaps become silence in the clip.
#[test]
fn the_audio_callback_does_no_io() {
    let audio = code("src/bin/clip-recorder/audio.rs", "//");
    let callback = between(&audio, "move |data: &[T]", "move |err: cpal::Error|");
    for forbidden in ["log(", "std::fs", "File", "println", "eprintln"] {
        assert!(!callback.contains(forbidden), "{forbidden} in the audio callback");
    }
}

/// PlaySoundW refuses SND_ASYNC with SND_MEMORY, so the play is synchronous: on its own thread,
/// or the press chime stalls the UI thread.
#[test]
fn chimes_never_play_on_the_calling_thread() {
    let chimes = code("src/bin/clip-recorder/chimes.rs", "//");
    assert!(position(&chimes, "std::thread::spawn") < position(&chimes, "PlaySoundW("));
    assert_eq!(chimes.matches("PlaySoundW(").count(), 1);
}

/// The job is joined before any ffmpeg exists, the probes included, or a crash orphans them.
#[test]
fn the_job_is_joined_before_the_first_ffmpeg() {
    let capture = code("src/bin/clip-recorder/capture.rs", "//");
    let new_in = between(&capture, "fn new_in(", "\n    }");
    assert!(position(new_in, "ensure_kill_on_close()") < position(new_in, "pipeline("));
}

/// One health tick heals the audio whatever the capture does: a tick that returned early for
/// a dead ffmpeg stopped a dead loopback from ever coming back.
#[test]
fn the_health_tick_never_returns_before_healing_the_audio() {
    let main = code("src/bin/clip-recorder/main.rs", "//");
    let tick = between(&main, "fn check_health()", "\n}");
    assert!(!tick.contains("return"), "{tick}");
    assert!(tick.contains("app.audio.heal()"));
}

/// Settings reads the device lists afresh on every open: one plugged in since launch must be
/// offered without a restart.
#[test]
fn settings_lists_devices_on_every_open() {
    let main = code("src/bin/clip-recorder/main.rs", "//");
    let open = between(&main, "fn open_settings()", "\n}");
    assert!(open.contains("audio::list_devices()"));
}

/// tests/regressions.md lists every fixed bug and its tests. A test it names must still exist:
/// deleting or renaming a guard has to update the registry in the same commit.
#[test]
fn every_fixed_bug_still_has_its_tests() {
    let registry = read("tests/regressions.md");
    let mut sources = String::new();
    for dir in ["src", "src/bin/clip-recorder", "tests"] {
        for entry in std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)).unwrap().flatten() {
            if entry.path().extension().is_some_and(|e| e == "rs") {
                sources.push_str(&std::fs::read_to_string(entry.path()).unwrap());
            }
        }
    }
    let rows: Vec<&str> = registry.lines().filter(|l| l.starts_with("| R")).collect();
    assert!(rows.len() >= 60, "the registry lost rows: {}", rows.len());
    for row in rows {
        let column = row.trim_end_matches('|').rsplit('|').next().unwrap_or("");
        let tests: Vec<&str> = column.split('`').skip(1).step_by(2).collect();
        assert!(!tests.is_empty(), "no test for {row}");
        for test in tests {
            assert!(sources.contains(&format!("fn {test}()")), "{test} (registry row {}) no longer exists", &row[..5]);
        }
    }
}
