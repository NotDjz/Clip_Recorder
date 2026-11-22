# Changelog

Every release of Clip Recorder, newest first. Sections are generated from the commit history by git-cliff.

## [2.2.0] - 2026-11-22

### Features

- Encode clips in HEVC 10-bit, FLAC audio in MKV
- Keep H.264 NVENC on older cards, tag HEVC MP4s hvc1
- Try DXGI with each NVENC codec before falling back to GDI

### Fixes

- Encode CPU clips in 4:2:0 so GPUs decode them

### Documentation

- Point the download page at v2.1.0

### CI and build

- Bump url to 2.5.9 for RUSTSEC-2026-0592

## [2.1.0] - 2026-11-21

### Features

- Choose MP4 or MKV for clips

### Documentation

- Point the download page at v2.0.4

### Tests

- Let the harness ask for an MKV clip
- Picking MP4 over an MKV config reads back MP4

## [2.0.4] - 2026-11-21

### Fixes

- Say when a clip has no game sound
- Reopen a loopback that stopped delivering
- Pick up a device plugged in after launch
- Read the screens again before a restart
- Find ddagrab's output by its rectangle
- Never kill a process started after its PID file
- Only a loopback that delivered all along can stall
- A found device that cannot open is not announced
- Ask DXGI only for a DXGI pipeline
- Remember the chosen screen by its name, and go back to it
- Show how the selected screen will really be captured
- Say when part of the game sound is missing
- Capture ddagrab on the NVIDIA adapter, named explicitly
- Keep the chosen screen unless another one is picked
- Read the form against the screens it showed
- Move to another screen only once the change holds 10 s
- Count each filled frame once
- Trim a hand-typed screen name

### Performance

- Fast-start only the clip itself

### Documentation

- Point the download page at v2.0.3
- Narrow R78 to what it fixes

### Tests

- Exclude two equivalent mutants

### CI and build

- Ignore RUSTSEC-2026-0543 (chrono unmaintained)

### Refactoring

- One wait-or-kill helper for every short ffmpeg
- Split the backoff out of decide
- One rule for which screen is captured
- The clock line belongs to the grabber

## [2.0.3] - 2026-11-15

### Features

- Check for updates from the tray menu and Settings

### Fixes

- Make the update link reachable and point it at every release

### Documentation

- Point the download page at v2.0.2

## [2.0.2] - 2026-11-15

### Features

- Log every panic, in any thread, before the default handling

### Fixes

- Never lose a panic report to the log's rotation
- Keep every unsigned-release notice true and in step

### Documentation

- Publish a code signing policy and announce start-up in the installer
- Show the Made with Slint badge on the download page
- Stop claiming the releases are signed
- Drop the plan to reapply to SignPath

### Tests

- Close the panic-log check the merge left open
- Show where the MSI lifecycle test hangs, and bound each msiexec
- Never let an msiexec in the lifecycle test reboot the runner
- Run the MSI lifecycle one phase per CI step

### CI and build

- Cargo-deny, an attested SBOM and WiX checked by hash
- Test the MSI's whole life on every change and before each release
- No release without the A/V sync measured on real hardware
- A CHANGELOG generated from the commits, and release notes from it
- Make the release scripts and steps runnable, and test them by running them
- Run the release's MSI lifecycle one phase per step, as CI does

## [2.0.1] - 2026-11-13

The first release of the Rust rewrite as the recommended version, replacing v1.7 (Python).

### Features

- Rust app installed by an MSI to Program Files, starting with Windows (switch in Settings, kept off across upgrades)
- Hardware encoding detected by a real test encode: DXGI into NVENC, else NVENC through GDI, else x264 through GDI
- A second launch from the Start Menu opens Settings in the running instance

### Fixes

- Audio is healed even while the capture restarts
- The configured audio device is matched exactly before by substring
- A pinned device that is unplugged is waited for instead of being replaced by the default for good
- Quits cleanly when Windows ends the session (an upgrade no longer crashes the app on its way out)
- Ends within 5 s of the end of the session even with a dialog open
- Two saves in the same second each get their own clip
- Save works on the first click and from the keyboard; text meets WCAG AA contrast
- A Python-era loopback device name is no longer listed twice in Settings
- Snapshots use the system copy and never overwrite or delete a file the save did not create
- The log is in UTC (RFC 3339) and temporary files have random names

### Measured

- A/V sync on the release build: median 0 ms at 60, 120 and 240 fps, no discontinuity

## [2.0] - 2026-11-13

Preview of the Rust rewrite, published as a pre-release.
