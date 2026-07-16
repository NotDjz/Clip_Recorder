# Clip Recorder

Replay screen recorder pour Windows — capture continue en arrière-plan, `Ctrl+Alt+R` sauvegarde les dernières secondes en MP4.

## Features

- Replay instantané (15-120s) avec audio système + micro
- GPU-accelerated (NVENC) + capture DXGI jusqu'à 240fps
- Sélection des devices audio dans les paramètres
- Portable — single exe, pas d'installation

## Install

Télécharger `ClipRecorder.exe` depuis [Releases](../../releases) ou `pip install -r requirements.txt && python clip_recorder.pyw`

Requires Windows 10/11, FFmpeg bundlé dans les releases.

## Build

`build.bat` télécharge FFmpeg, génère l'icône puis lance PyInstaller (`--onefile --windowed`). L'exe est produit dans `dist/`.

## License

[MIT](LICENSE)
