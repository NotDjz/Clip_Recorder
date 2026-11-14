# Clip Recorder

Replay screen recorder for Windows: it keeps the last few seconds of your screen and sound in the background, and
`Ctrl+Alt+R` saves them as an MP4. An open-source alternative to ShadowPlay or Medal.

## Features

- Instant replay of the last 15 to 120 s, with system sound and microphone
- DXGI capture into NVENC up to 240 fps; without an NVIDIA GPU it records on the CPU at up to 60 fps
- Audio and video in sync (measured median 0 ms at 60, 120 and 240 fps)
- No overlay in your game: a tray icon and a short notification that is never captured into clips

## Install

Download `clip-recorder-<version>-x64.msi` from [Releases](../../releases) and run it. It installs to Program Files
and starts with Windows (switch that off in Settings). Uninstall it from Windows Settings > Apps; your clips stay.

Requires Windows 10 or 11. FFmpeg is included.

## Code signing policy

See [CODE_SIGNING_POLICY.md](CODE_SIGNING_POLICY.md).

## Build

Rust (version pinned in `rust-toolchain.toml`) and WiX 7.

- `cargo test --locked` runs the tests; `pwsh ./build-msi.ps1` fetches the hash-pinned FFmpeg, builds the release
  exe and produces `target\clip-recorder-<version>-x64.msi`.
- Releases: `pwsh scripts/prepare-release.ps1 <version>` opens a PR with the version bump and its
  [CHANGELOG.md](CHANGELOG.md) section (git-cliff, from the Conventional Commits); once merged, a `v<version>` tag
  makes `.github/workflows/release.yml` build, test and attest the MSI and its SBOM, and publish after approval.

Every bug fixed so far is listed with the tests that guard it in [tests/regressions.md](tests/regressions.md).

## Acknowledgements

- The Settings window is built with [Slint](https://slint.dev), under the Slint Royalty-free License 2.0; the
  attribution badge is on the download page.
- [FFmpeg](https://ffmpeg.org) does the encoding (`ffmpeg.exe`, GPL v3, gyan.dev build), bundled unmodified.

## License

[MIT](LICENSE)
