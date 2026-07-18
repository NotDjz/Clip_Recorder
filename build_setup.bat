@echo off
if not exist dist\ClipRecorder.exe (
    echo dist\ClipRecorder.exe introuvable - lancez build.bat d'abord.
    pause
    exit /b 1
)
if not exist icon.ico py generate_icon.py
py -m PyInstaller --noconfirm --onefile --windowed --name ClipRecorderSetup ^
    --icon=icon.ico ^
    --add-data "dist\ClipRecorder.exe;." ^
    setup.pyw
echo.
echo Build termine : dist\ClipRecorderSetup.exe
pause
