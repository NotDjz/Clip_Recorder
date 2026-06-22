@echo off
py download_ffmpeg.py
py generate_icon.py
py -m PyInstaller --noconfirm --onefile --windowed --name ClipRecorder ^
    --icon=icon.ico ^
    --add-data "ffmpeg.exe;." ^
    --hidden-import pystray._win32 ^
    clip_recorder.pyw
echo.
echo Build termine : dist\ClipRecorder.exe
pause
