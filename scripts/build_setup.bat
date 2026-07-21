@echo off
REM Must run AFTER build.bat — it embeds the app exe it produced.
cd /d %~dp0..
if not exist dist\ClipRecorder.exe (
    echo dist\ClipRecorder.exe introuvable - lancez build.bat d'abord.
    pause
    exit /b 1
)
if not exist assets\icon.ico py scripts\generate_icon.py
if not exist assets\icon.ico goto :fail

if exist dist\ClipRecorderSetup.exe del /q dist\ClipRecorderSetup.exe

py -m PyInstaller --noconfirm --onefile --windowed --name ClipRecorderSetup ^
    --icon=assets\icon.ico ^
    --add-data "dist\ClipRecorder.exe;." ^
    src\setup.pyw || goto :fail
if not exist dist\ClipRecorderSetup.exe goto :fail

echo.
echo Build termine : dist\ClipRecorderSetup.exe
pause
exit /b 0

:fail
echo.
echo BUILD ECHOUE - dist\ClipRecorderSetup.exe n'a pas ete produit.
pause
exit /b 1
