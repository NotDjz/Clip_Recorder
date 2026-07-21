@echo off
REM Call by absolute path: each script cd's to the repo root itself, so relying
REM on the current directory here would break the second call.
call "%~dp0build.bat"
call "%~dp0build_setup.bat"
