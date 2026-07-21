@echo off
REM Call by absolute path: each script cd's to the repo root itself, so relying
REM on the current directory here would break the second call.
call "%~dp0build.bat"
if errorlevel 1 (
    echo.
    echo build.bat a echoue - build_setup.bat n'est pas lance ^(il embarquerait un exe perime^).
    exit /b 1
)
call "%~dp0build_setup.bat"
exit /b %errorlevel%
