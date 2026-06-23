# Clip Recorder

Lightweight replay screen recorder for Windows — captures the last X seconds of your screen + audio as MP4 via a single hotkey. No overlay, no bloatware.

Think ShadowPlay / Medal, but open-source and minimal.

## Features

- **Instant replay** — continuously records in the background, `Ctrl+Alt+R` saves the last N seconds
- **System audio + microphone** — captured automatically via WASAPI loopback (pyaudiowpatch), zero configuration
- **GPU-accelerated** — uses NVENC (NVIDIA) when available, falls back to x264 CPU encoding
- **High FPS capture** — up to 240 FPS via DXGI Desktop Duplication (ddagrab), with GDI fallback for compatibility
- **Multi-monitor support** — select which screen to capture in settings
- **Portable** — single exe, config stored next to it, no install required
- **System tray** — runs silently in the background, right-click for options

## Installation

### Binary (recommended)

Download `ClipRecorder.exe` from [Releases](../../releases) and place it in any folder. FFmpeg is bundled — no external dependencies needed.

### From source

Requires Python 3.10+ and [FFmpeg](https://ffmpeg.org/download.html) (`ffmpeg.exe` in the same folder or in PATH).

```bash
pip install -r requirements.txt
python clip_recorder.pyw
```

## Usage

1. Launch `ClipRecorder.exe` — capture starts automatically
2. A red dot appears in the system tray
3. Press **`Ctrl+Alt+R`** to save a clip
4. A "Clip enregistre" banner appears for 3 seconds
5. Right-click the tray icon for settings, folder access, or to quit

### Settings

- **Screen** — which monitor to capture
- **FPS** — 30, 60, 120, or 240 (120/240 require ddagrab support)
- **Buffer duration** — 15, 30, 60, 90, or 120 seconds
- **Output folder** — where clips are saved (default: `Clips/` next to the exe)

## Build

```bash
pip install -r requirements.txt
python download_ffmpeg.py
python generate_icon.py
build.bat
```

The exe is generated in `dist/ClipRecorder.exe`.

## How it works

FFmpeg runs continuously, writing rolling MPEG-TS segments to a temp directory. When you press the hotkey, the app concatenates the most recent segments into an MP4, muxes in the audio from WASAPI circular buffers, and saves the result. The capture is never interrupted.

## Requirements

- Windows 10/11
- FFmpeg 5+ (bundled in releases, or bring your own)
- NVIDIA GPU recommended (for NVENC + ddagrab), but not required

## License

[MIT](LICENSE)
