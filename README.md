# Clip Recorder

Replay screen recorder for Windows — continuous background capture, `Ctrl+Alt+R` saves the last few seconds as MP4.

## Features

- Instant replay (15-120s) with system audio + mic
- GPU-accelerated (NVENC) + DXGI capture up to 240fps
- Audio device selection in settings
- Portable — single exe, no installation

## Install

Download `ClipRecorder.exe` from [Releases](../../releases) or `pip install -r requirements.txt && python clip_recorder.pyw`

Requires Windows 10/11, FFmpeg bundled in releases.

## Build

`build.bat` downloads FFmpeg, generates the icon, then runs PyInstaller (`--onefile --windowed`). The exe is produced in `dist/`.

## License

[MIT](LICENSE)
