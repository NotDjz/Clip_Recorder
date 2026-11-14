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
    let len = text[start..]
        .find(to)
        .unwrap_or_else(|| panic!("{to:?} not found after {from:?}"));
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
    assert!(
        run.contains(r#"Root="HKCU""#) && run.contains(r"CurrentVersion\Run"),
        "{run}"
    );
    assert!(
        !wxs.contains("CloseApplication"),
        "util:CloseApplication blocks upgrades"
    );
    assert!(wxs.contains("<MajorUpgrade"), "an upgrade must replace the old install");
}

/// The shipped ffmpeg is the pinned one: checked before it is moved into place, an unverified
/// one deleted first, and the MSI never built after a failed cargo build (it once shipped the
/// previous exe).
#[test]
fn the_build_ships_only_a_verified_ffmpeg_and_a_fresh_exe() {
    let ps = code("build-msi.ps1", "#");
    assert!(
        read("build-msi.ps1").starts_with("#Requires -Version 7"),
        "PowerShell 5 stops before building"
    );
    let check = position(&ps, "if (-not (Test-Pinned $part))");
    assert!(
        check < position(&ps, "Move-Item $part $ffmpeg"),
        "moved before its hash was checked"
    );
    assert!(
        position(&ps, "Remove-Item $ffmpeg") < position(&ps, "Invoke-WebRequest"),
        "a stale exe stays where the build looks"
    );
    let built = position(&ps, "cargo build --release --locked");
    let stop = position(&ps, "if ($LASTEXITCODE) { exit $LASTEXITCODE }");
    assert!(
        built < stop && stop < position(&ps, "wix build"),
        "wix runs after a failed cargo build"
    );
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
        for entry in std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir))
            .unwrap()
            .flatten()
        {
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
            assert!(
                sources.contains(&format!("fn {test}()")),
                "{test} (registry row {}) no longer exists",
                &row[..5]
            );
        }
    }
}

/// Every third-party action is pinned to a full commit SHA (a tag can be moved under us), and
/// every workflow starts from a read-only token.
#[test]
fn workflows_pin_their_actions_and_start_read_only() {
    for workflow in [".github/workflows/ci.yml", ".github/workflows/release.yml"] {
        let text = code(workflow, "#");
        let uses: Vec<&str> = text.lines().filter_map(|l| l.trim().strip_prefix("- uses: ")).collect();
        assert!(!uses.is_empty(), "{workflow}");
        for action in uses {
            let sha = action
                .split('@')
                .nth(1)
                .and_then(|r| r.split_whitespace().next())
                .unwrap_or("");
            assert!(
                sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
                "{workflow}: {action}"
            );
        }
        let top = text.split("\njobs:").next().unwrap();
        assert!(
            top.contains("permissions:\n  contents: read"),
            "{workflow} must default to a read-only token"
        );
        assert!(
            !text.contains("pull_request_target"),
            "{workflow} would run a fork's code with secrets"
        );
    }
}

/// The session window only asks the event loop to stop: process::exit inside a dispatched
/// message ran the UI's destructors mid-dispatch and crashed on the way out (0xC0000409).
#[test]
fn the_session_handler_leaves_through_main() {
    let session = code("src/bin/clip-recorder/session.rs", "//");
    assert!(
        !session.contains("process::exit"),
        "exit through main(), not from the handler"
    );
    assert!(session.contains("slint::quit_event_loop()"));
    // A modal loop (folder picker, message box) keeps main() from resuming: a deadline ends it.
    let handler = &session[session.find("WM_ENDSESSION").unwrap()..];
    assert!(
        handler.contains("TerminateProcess(GetCurrentProcess(), 0)"),
        "no deadline after the quit request"
    );
}

/// SignPath Foundation signs only projects that publish its code signing policy: the exact
/// attribution, the team roles and the privacy statement, linked from the home and download
/// pages, and an installer that announces the system changes it makes (start with Windows).
#[test]
fn the_code_signing_policy_is_published_where_signpath_requires_it() {
    let policy = read("CODE_SIGNING_POLICY.md");
    for required in [
        "Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org)",
        "Committers and reviewers:",
        "Approvers:",
        "This program will not transfer any information to other networked systems unless specifically requested by the user",
    ] {
        assert!(policy.contains(required), "CODE_SIGNING_POLICY.md lacks: {required}");
    }
    for page in ["README.md", "docs/index.html", ".github/workflows/release.yml"] {
        assert!(
            read(page).contains("Code signing policy"),
            "{page} does not link the code signing policy"
        );
    }
    assert!(
        read("installer/license.rtf").contains("start with Windows"),
        "the MSI does not announce it"
    );
}

/// A panic in the windowed app (no console) must reach the log: the hook is installed before
/// anything else in main(), and it never blocks on the log lock the panicking thread may hold.
#[test]
fn panics_are_logged_from_the_first_line_of_main() {
    let main = code("src/bin/clip-recorder/main.rs", "//");
    let body = between(&main, "fn main() {", "\n}");
    let first = body.lines().nth(1).unwrap_or("").trim();
    assert_eq!(
        first, "logging::install_panic_hook();",
        "not the first statement of main()"
    );
    let logging = code("src/bin/clip-recorder/logging.rs", "//");
    assert!(between(&logging, "fn install_panic_hook()", "\n}").contains("write(&report, true)"));
    assert!(
        logging.contains("LOCK.try_lock()"),
        "the panic path waits on the log lock"
    );
}

/// Slint is used under its Royalty-free License 2.0, whose condition (section 2) is the
/// "Made with Slint" badge on a public page, preferably the download page: removing it breaks the
/// licence of every release.
#[test]
fn the_download_page_shows_the_made_with_slint_badge() {
    let page = read("docs/index.html");
    let download = &page[position(&page, "Download for Windows")..];
    assert!(download.contains("made-with-slint-light.svg") && download.contains("alt=\"Made with Slint\""));
    for badge in ["docs/made-with-slint-light.svg", "docs/made-with-slint-dark.svg"] {
        assert!(read(badge).contains("<svg"), "{badge} missing");
    }
}

/// Dependencies come from crates.io only, with no wildcard versions, and WiX, the one tool the
/// release installs from outside crates.io, is used only after its packages' published hashes match.
#[test]
fn the_supply_chain_is_pinned_and_verified() {
    let deny = code("deny.toml", "#");
    assert!(deny.contains("allow-git = []") && deny.contains("unknown-git = \"deny\""));
    assert!(deny.contains("wildcards = \"deny\""));
    let wix = code("scripts/install-wix.ps1", "#");
    assert!(
        position(&wix, "HashData") < position(&wix, "Expand-Archive"),
        "WiX unpacked before its hash was checked"
    );
    let release = code(".github/workflows/release.yml", "#");
    assert!(release.contains("./scripts/install-wix.ps1"));
    assert!(
        !release.contains("dotnet tool install"),
        "WiX installed unchecked from NuGet"
    );
    assert!(release.contains("actions/attest-sbom@"), "the SBOM is not attested");
    assert!(code(".github/workflows/ci.yml", "#").contains("cargo deny --locked check"));
}

/// The installer's life (install, upgrade with the app running, uninstall) is tested on a
/// disposable machine in CI, and again on the very MSI a release is about to publish.
#[test]
fn every_msi_goes_through_its_lifecycle_test() {
    for workflow in [".github/workflows/ci.yml", ".github/workflows/release.yml"] {
        let text = code(workflow, "#");
        assert!(
            text.contains("./tests/msi/lifecycle.ps1"),
            "{workflow} skips the MSI lifecycle test"
        );
    }
    let release = code(".github/workflows/release.yml", "#");
    assert!(
        position(&release, "./tests/msi/lifecycle.ps1") < position(&release, "actions/attest-build-provenance@"),
        "the release attests an MSI before testing it"
    );
}

/// A/V sync, the most expensive bug of the project's history, needs real sound to measure: no
/// release is built until scripts/release-check.ps1 has measured the tagged commit at the computer.
#[test]
fn no_release_without_the_measured_av_sync() {
    let release = code(".github/workflows/release.yml", "#");
    let gate = between(&release, "av-sync:", "build:");
    assert!(
        gate.contains("release/av-sync") && gate.contains("exit 1"),
        "the release does not check the measurement"
    );
    assert!(
        between(&release, "build:", "steps:").contains("needs: av-sync"),
        "the build does not wait for it"
    );
    let check = code("scripts/release-check.ps1", "#");
    for fps in ["60", "120", "240"] {
        assert!(check.contains(fps), "release-check does not measure {fps} fps");
    }
    assert!(check.contains("context=release/av-sync"));
}

/// Release notes come from CHANGELOG.md (written by scripts/prepare-release.ps1 from the
/// Conventional Commits): a release whose version has no section there stops before publishing.
#[test]
fn release_notes_are_the_changelog_section() {
    let release = code(".github/workflows/release.yml", "#");
    assert!(
        release.contains("CHANGELOG.md has no section for"),
        "a release without its changelog section"
    );
    assert!(release.contains("--notes-file notes.md") && !release.contains("--generate-notes"));
    assert!(read("CHANGELOG.md").contains("## [2.0.1]"));
}
