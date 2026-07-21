@echo off
REM Always build from the repo root: ffmpeg.exe, build\ and dist\ live there.
cd /d %~dp0..
py scripts\download_ffmpeg.py
py scripts\generate_icon.py
py -m PyInstaller --noconfirm --onefile --windowed --name ClipRecorder ^
    --icon=assets\icon.ico ^
    --add-data "ffmpeg.exe;." ^
    --hidden-import pystray._win32 ^
    src\clip_recorder.pyw
echo.
echo Build termine : dist\ClipRecorder.exe
pause
