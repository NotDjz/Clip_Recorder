@echo off
REM Always build from the repo root: ffmpeg.exe, build\ and dist\ live there.
cd /d %~dp0..
py scripts\download_ffmpeg.py || goto :fail
py scripts\generate_icon.py || goto :fail

REM Delete any previous exe FIRST. build_setup.bat embeds whatever it finds in
REM dist\, and that installer is the only asset published — so a failed build
REM here must leave no stale exe behind for it to pick up.
if exist dist\ClipRecorder.exe del /q dist\ClipRecorder.exe

py -m PyInstaller --noconfirm --onefile --windowed --name ClipRecorder ^
    --icon=assets\icon.ico ^
    --add-data "ffmpeg.exe;." ^
    --hidden-import pystray._win32 ^
    src\clip_recorder.pyw || goto :fail
if not exist dist\ClipRecorder.exe goto :fail

echo.
echo Build termine : dist\ClipRecorder.exe
pause
exit /b 0

:fail
echo.
echo BUILD ECHOUE - dist\ClipRecorder.exe n'a pas ete produit.
pause
exit /b 1
