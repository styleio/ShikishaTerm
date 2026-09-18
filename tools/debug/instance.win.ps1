<#
  Start a copy of the app that is nobody's: its own folder, its own settings,
  its own door, and print what is needed to drive it.

  Two agents debugging at once is the case this exists for. One running copy per
  layout is the rule (crates/core/src/instance.rs), and "the same layout" means
  the same folder -- so a copy of its own is what makes a second agent possible
  at all. Everything that could collide comes with it: the settings, the state,
  the pipe's name (it carries the process id) and the board's port.

  Nothing of a copy somebody is using is read, written or stopped. The folder
  somebody works in is not touched, and neither is what is installed.

    -Exe      the build to run (default: this checkout's target\debug)
    -At       where to put the copy (default: %TEMP%\sk-instance)
    -Port     serve the board on this port too, for driving the screen from afar
    -Cdp      open the window's own DevTools port, for driving its page directly
    -Work     a folder for its one tab to sit in (default: a folder inside -At)
    -Stop     stop the copy at -At and leave

  What it prints is meant to be read by whoever started it:

    root=C:\...\sk-instance\app
    pid=12345
    pipe=\\.\pipe\shikisha-12345
    token-file=C:\...\sk-instance\app\data\api-token
    mcp=... --mcp --pid 12345 --token-file ...

  The last line is the command an MCP client is given. The key is left in the
  file for it to read; it is not printed, because a command line is visible to
  every process on the machine.
#>
param(
    [string]$Exe,
    [string]$At = (Join-Path $env:TEMP 'sk-instance'),
    [int]$Port = 0,
    [int]$Cdp = 0,
    [string]$Work,
    [switch]$Stop
)
$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $PSScriptRoot          # tools
$root = Split-Path -Parent $here                  # the checkout
$app = Join-Path $At 'app'
if (-not $Exe) { $Exe = Join-Path $root 'target\debug\SHIKISHA-TERM.exe' }
if (-not $Work) { $Work = Join-Path $At 'work' }

# Only the copies under this folder. A copy somebody is using lives elsewhere
# and is none of this script's business -- which is the whole point of naming
# the folder rather than the program
function Stop-Copy {
    Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path -like (Join-Path $At '*') } |
        ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }
}

if ($Stop) {
    Stop-Copy
    Write-Host "stopped=$At"
    return
}

if (-not (Test-Path $Exe)) { throw "no build at $Exe -- run cargo build first" }

Stop-Copy
Start-Sleep -Milliseconds 800
if (Test-Path $At) { Remove-Item -Recurse -Force $At }
foreach ($d in @($app, $Work, (Join-Path $At 'localappdata'))) {
    New-Item -ItemType Directory -Force -Path $d | Out-Null
}

# The payload beside the exe (profiles, lang, the rest of dist.list) travels by
# the one copier, so a new file reaches this the same day it reaches a release
& (Join-Path $here 'stage.ps1') -Dest $app -Package -Exe $Exe | Out-Null
if (-not (Test-Path (Join-Path $app 'SHIKISHA-TERM.exe'))) { throw "staging $app failed" }

# `access: user` is what leaves the key in data\api-token for a script to read.
# It is given to this copy only: it is made for this run, thrown away after it,
# and holds nothing of anybody's.
$settings = [ordered]@{
    language      = 'en'
    external_api  = [ordered]@{ access = 'user' }
    remote        = [ordered]@{ enabled = ($Port -gt 0); bind = '127.0.0.1'; port = $Port }
    desks         = @([ordered]@{
        name    = 'Check'
        id      = 'check'
        folders = @([ordered]@{
            cwd  = $Work
            tabs = @([ordered]@{ name = 'shell'; id = 'shell'; command = 'cmd.exe' })
        })
    })
}
$cfgDir = Join-Path $app 'config'
New-Item -ItemType Directory -Force -Path $cfgDir | Out-Null
$settings | ConvertTo-Json -Depth 8 | Set-Content -Path (Join-Path $cfgDir 'config.json') -Encoding UTF8

# Started with an environment of its own, and started in a way that hands it
# none of this shell's handles: a copy that inherited them would hold the
# caller's output open, and whoever ran this would sit there waiting for a
# window to be closed.
#
# The agent's own variables are taken away for the moment of the launch. A
# Claude Code started inside this copy would otherwise inherit them and be
# taken for a child of the session that started the copy, which changes how it
# behaves (and is a mistake that has been made here before).
$aside = @{}
foreach ($e in [System.Environment]::GetEnvironmentVariables().GetEnumerator()) {
    if ([string]$e.Key -match '^(CLAUDE|ANTHROPIC|SHIKISHA)') { $aside[[string]$e.Key] = [string]$e.Value }
}
$aside['LOCALAPPDATA'] = $env:LOCALAPPDATA
$proc = $null
try {
    # Taken away, not blanked: a variable set to nothing is still a variable
    # that is set, and what reads these asks whether they are there
    foreach ($k in @($aside.Keys)) { Remove-Item -Path "env:$k" -ErrorAction SilentlyContinue }
    $env:LOCALAPPDATA = Join-Path $At 'localappdata'
    if ($Cdp -gt 0) { $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Cdp" }
    $proc = Start-Process -FilePath (Join-Path $app 'SHIKISHA-TERM.exe') -WorkingDirectory $app -PassThru
} finally {
    foreach ($k in @($aside.Keys)) {
        if ([string]::IsNullOrEmpty($aside[$k])) { Remove-Item -Path "env:$k" -ErrorAction SilentlyContinue }
        else { Set-Item -Path "env:$k" -Value $aside[$k] }
    }
    Remove-Item -Path 'env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS' -ErrorAction SilentlyContinue
}
if (-not $proc) { throw 'the copy did not start' }

# The key is written as the door opens, so its arrival is what says the copy is
# up -- a better answer than a fixed wait, which is either wrong or slow
$tokenFile = Join-Path $app 'data\api-token'
$waited = 0
while (-not (Test-Path $tokenFile) -and $waited -lt 30000) {
    if ($proc.HasExited) { throw "the copy stopped before its door opened (exit $($proc.ExitCode))" }
    Start-Sleep -Milliseconds 250
    $waited += 250
}
if (-not (Test-Path $tokenFile)) { Stop-Copy; throw "the door never opened -- see $app\logs" }

$mcpExe = Join-Path $app 'SHIKISHA-TERM.exe'
Write-Host "root=$app"
Write-Host "work=$Work"
Write-Host "pid=$($proc.Id)"
Write-Host "pipe=\\.\pipe\shikisha-$($proc.Id)"
Write-Host "token-file=$tokenFile"
if ($Port -gt 0) { Write-Host "board=http://127.0.0.1:$Port" }
if ($Cdp -gt 0) { Write-Host "cdp=http://127.0.0.1:$Cdp" }
Write-Host "mcp=`"$mcpExe`" --mcp --pid $($proc.Id) --token-file `"$tokenFile`""
