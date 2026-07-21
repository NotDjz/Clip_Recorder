# Clip Recorder

Replay screen recorder for Windows — continuous background capture, `Ctrl+Alt+R` saves the last few seconds as MP4.

## Features

- Instant replay (15-120s) with system audio + mic
- GPU-accelerated (NVENC) + DXGI capture up to 240fps
- Audio device selection in settings
- Portable app — no admin rights, no registry, uninstall from Settings

## Install

Download `ClipRecorderSetup.exe` from [Releases](../../releases) and run it — pick a folder, optionally create a desktop shortcut. Or run from source: `pip install -r requirements.txt && python src\clip_recorder.pyw`

Requires Windows 10/11, FFmpeg bundled in releases.

## Build

`scripts\build.bat` downloads FFmpeg, generates the icon, then runs PyInstaller (`--onefile --windowed`) to produce `dist\ClipRecorder.exe`. `scripts\build_setup.bat` (run after) bundles that exe into `dist\ClipRecorderSetup.exe`. `scripts\build_all.bat` runs both in order.

Layout: `src/` app + installer, `scripts/` build tooling, `tests/` test harnesses, `assets/` icon, `docs/` notes.

## License

[MIT](LICENSE)
