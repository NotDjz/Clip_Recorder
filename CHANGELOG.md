# Changelog

Every release of Clip Recorder, newest first. Sections are generated from the commit history by git-cliff.

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
