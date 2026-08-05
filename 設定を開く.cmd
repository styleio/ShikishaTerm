@echo off
rem Opens only the settings screen in your browser (no AI tabs).
rem Also useful to recover when a broken config prevents startup.
cd /d "%~dp0"
start "" "%~dp0Shikisha-Term-AI.exe" --settings
