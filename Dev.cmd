@echo off
rem Build, stage into run\, and launch from there.
rem
rem Config and scripts live next to the executable, so launching
rem target\debug and target\release directly gives you two separate sets of
rem settings -- an automation deleted in one is still alive in the other, and
rem that is painful to track down. This pins the run location to run\.
rem
rem   Dev.cmd                 build debug and run
rem   Dev.cmd release         build release and run
rem   Dev.cmd setup           only prepare run\ (do not launch)
rem   Dev.cmd ssh user@host   extra arguments are passed to the app
rem
rem ASCII only on purpose: cmd reads .cmd in the OEM code page, so UTF-8
rem Japanese in here breaks parsing.
setlocal EnableExtensions
cd /d "%~dp0"

set "PROFILE=debug"
set "FLAGS="
set "SETUPONLY="

rem Keep taking option words, in any order, until something else shows up
:parse
if /i "%~1"=="release" goto opt_release
if /i "%~1"=="setup" goto opt_setup
goto collect

:opt_release
set "PROFILE=release"
set "FLAGS=--release"
shift
goto parse

:opt_setup
set "SETUPONLY=1"
shift
goto parse

rem Remaining arguments go to the app (%* is unusable after shift)
:collect
set "ARGS="
:collect_loop
if "%~1"=="" goto build
set "ARGS=%ARGS% %1"
shift
goto collect_loop

:build
rem Microsoft's newer ConPTY, fetched once and verified against a pinned hash.
rem Without it the terminal falls back to the one in Windows, which is slower
rem and drops part of what programs send -- and says nothing about it. A
rem failure here is not fatal: you get a warning and the older engine.
powershell -NoProfile -ExecutionPolicy Bypass -File "tools\conpty.ps1"

cargo build %FLAGS%
if errorlevel 1 exit /b 1

rem First time only: carry over settings from target\debug, which is where
rem they ended up if you had been launching the build output directly.
rem The originals are left in place, so a mix-up is recoverable.
if exist run goto have_run
mkdir run
if not exist "target\debug\config.json" goto have_run
echo Carrying over existing settings from target\debug
copy /y "target\debug\config.json" "run\" >nul
if exist "target\debug\secrets.json" copy /y "target\debug\secrets.json" "run\" >nul
if exist "target\debug\scripts" xcopy /y /e /i /q "target\debug\scripts" "run\scripts" >nul
if exist "target\debug\workspaces" xcopy /y /e /i /q "target\debug\workspaces" "run\workspaces" >nul

:have_run
rem Application files are refreshed every time, so a change you just made is
rem certain to be the one running.
copy /y "target\%PROFILE%\SHIKISHA-TERM.exe" "run\" >nul
if errorlevel 1 (
  echo Executable not found: target\%PROFILE%
  exit /b 1
)
copy /y "Settings.cmd" "run\" >nul
rem What travels with the exe is dist.list's to decide -- the same list the
rem release build and build.rs read. This file used to name lang, profiles and
rem docs itself, which is exactly the arrangement that let conpty.dll reach the
rem download and not the machine it was developed on.
powershell -NoProfile -ExecutionPolicy Bypass -File "tools\stage.ps1" -Dest "run" >nul
if errorlevel 1 exit /b 1

rem Your own data is never overwritten
if not exist "run\config.json" copy /y "config.example.json" "run\config.json" >nul

if defined SETUPONLY (
  echo Prepared run\ from %PROFILE%
  exit /b 0
)

cd run
SHIKISHA-TERM.exe%ARGS%
