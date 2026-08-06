@echo off
rem Launch in its own window. Double-clicking the exe uses the terminal instead.
cd /d "%~dp0"
start "" "%~dp0SHIKISHA-TERM.exe" --window
