<#
  Start a copy of the app that is nobody's: its own folder, settings, state,
  door and ports, and print what is needed to drive it.

  Two agents debugging at once is the case this exists for. One running copy per
  layout is the rule (crates/core/src/instance.rs), and "the same layout" means
  the same folder -- so a copy of its own is what makes a second agent possible
  at all. Everything that could collide comes with it: the settings, the state,
  the pipe's name (it carries the process id) and the ports.

  Nothing of a copy somebody is using is read, written or stopped. The folder
  somebody works in is not touched, and neither is what is installed.

    -Exe      the build to run (default: this checkout's target\debug)
    -At       where to put the copy (default: %TEMP%\sk-instance)
    -Work     a folder for its one tab to sit in (default: a folder inside -At)
    -Port     the board's port. Left out, one is taken from the band below; 0 is no board
    -Cdp      the window's DevTools port, for driving its page directly. Same rule
    -Mcp      where to write the client's MCP settings (default: <At>\mcp.json)
    -Name     what the server is called in those settings (default: shikisha)
    -Stop     stop the copy at -At and leave

  **Ports come from 9400-9499**, two at a time: an even one for the board and
  the odd one after it for DevTools. The first free pair is taken, so two of
  these running at once cannot land on each other, and the band is clear of
  what the other tools here use (93xx) and of the demo (8788).

  What it prints is meant to be read by whoever started it:

    root=C:\...\sk-instance\app
    pid=12345
    pipe=\\.\pipe\shikisha-12345
    token-file=C:\...\sk-instance\app\data\api-token
    board=http://127.0.0.1:9400
    cdp=http://127.0.0.1:9401
    mcp-config=C:\...\sk-instance\mcp.json
    mcp=... --mcp --pid 12345 --token-file ...

  The key itself is never printed: a command line is visible to every process
  on the machine. It is left in the file for a client to read.
#>
param(
    [string]$Exe,
    [string]$At = (Join-Path $env:TEMP 'sk-instance'),
    [string]$Work,
    [int]$Port,
    [int]$Cdp,
    [string]$Mcp,
    [string]$Name = 'shikisha',
    [switch]$Stop
)
$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $PSScriptRoot          # tools
$root = Split-Path -Parent $here                  # the checkout
$app = Join-Path $At 'app'
if (-not $Exe) { $Exe = Join-Path $root 'target\debug\SHIKISHA-TERM.exe' }
if (-not $Work) { $Work = Join-Path $At 'work' }
if (-not $Mcp) { $Mcp = Join-Path $At 'mcp.json' }

# Only the copies under this folder. A copy somebody is using lives elsewhere
# and is none of this script's business -- which is the whole point of naming
# the folder rather than the program
function Stop-Copy {
    Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path -like (Join-Path $At '*') } |
        ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }
}

# Free means nothing is listening on the loopback now, and the way to ask is to
# take it for a moment: whether taking it works is the answer
function Test-PortFree([int]$p) {
    try {
        $l = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $p)
        $l.Start(); $l.Stop()
        return $true
    } catch { return $false }
}

# The client's settings file as something a server can be added to: the object
# it holds, or an empty one when there is no file yet.
#
# Asked, never assumed. A failed parse that is allowed to pass for "no file"
# reads exactly like an empty one, and what gets written then stands where
# somebody else's settings were.
function Read-McpDoc([string]$path) {
    if (-not (Test-Path $path)) { return [pscustomobject]@{} }
    $text = Get-Content $path -Raw -Encoding UTF8
    if (-not $text -or -not $text.Trim()) { return [pscustomobject]@{} }
    $doc = $null
    try { $doc = $text | ConvertFrom-Json -ErrorAction Stop } catch { $doc = $null }
    if ($doc -isnot [pscustomobject]) {
        throw "$path holds something this cannot add a server to -- name another file with -Mcp rather than have this overwrite it"
    }
    return $doc
}

# A port taken a moment ago can be gone by the time the app asks for it.
# Printing an address that answers nothing would send whoever reads that line
# looking for a fault in their own client, so it is checked before it is said
function Wait-Port([int]$p, [string]$what) {
    for ($i = 0; $i -lt 40; $i++) {
        try {
            $c = [System.Net.Sockets.TcpClient]::new()
            $c.Connect([System.Net.IPAddress]::Loopback, $p)
            $c.Close()
            return
        } catch { Start-Sleep -Milliseconds 250 }
    }
    Stop-Copy
    throw "$what never answered on $p -- something else may hold it (see $app\logs)"
}

if ($Stop) {
    Stop-Copy
    Write-Host "stopped=$At"
    return
}

if (-not (Test-Path $Exe)) { throw "no build at $Exe -- run cargo build first" }

# Asked before anything is started or staged. Finding out at the end that the
# settings cannot be written would leave a copy running that nobody was told
# the details of
[void](Read-McpDoc $Mcp)

Stop-Copy
Start-Sleep -Milliseconds 800

# The pair, unless the caller named the ports. Asked for after the copy at this
# folder is gone, so restarting one takes its own ports back instead of walking
# up the band every time. Picked together so that a copy's two ports are always
# neighbours -- which is what makes a stray port on this machine traceable back
# to the copy it belongs to
if (-not $PSBoundParameters.ContainsKey('Port') -or -not $PSBoundParameters.ContainsKey('Cdp')) {
    $pair = $null
    for ($p = 9400; $p -le 9498; $p += 2) {
        if ((Test-PortFree $p) -and (Test-PortFree ($p + 1))) { $pair = $p; break }
    }
    if (-not $pair) { throw 'every port in 9400-9499 is busy -- stop a copy you are not using' }
    if (-not $PSBoundParameters.ContainsKey('Port')) { $Port = $pair }
    if (-not $PSBoundParameters.ContainsKey('Cdp')) { $Cdp = $pair + 1 }
}
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
# No byte-order mark: PowerShell 5.1 puts one on with -Encoding UTF8, and a
# mark in front of a JSON file is a parse error for most readers. The app
# happens to strip one; a client reading the settings below may not
$noBom = [System.Text.UTF8Encoding]::new($false)
[System.IO.File]::WriteAllText((Join-Path $cfgDir 'config.json'), ($settings | ConvertTo-Json -Depth 8), $noBom)

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

if ($Port -gt 0) { Wait-Port $Port 'the board' }
if ($Cdp -gt 0) { Wait-Port $Cdp 'the DevTools port' }

# The settings a client reads to find this copy. Written at every start,
# because the door's name carries the process id and that is new each time.
#
# Merged, never overwritten: the file may be a worktree's own `.mcp.json` with
# other servers in it, and taking those away because this one moved would be a
# surprise nobody asked for. A file that is there and is not JSON is left alone
# and said out loud -- it belongs to something else.
$mcpExe = Join-Path $app 'SHIKISHA-TERM.exe'
$entry = [ordered]@{
    command = $mcpExe
    args    = @('--mcp', '--pid', "$($proc.Id)", '--token-file', $tokenFile)
}
$doc = Read-McpDoc $Mcp
if (-not $doc.PSObject.Properties['mcpServers']) {
    $doc | Add-Member -NotePropertyName mcpServers -NotePropertyValue ([pscustomobject]@{})
}
$servers = $doc.mcpServers
if ($servers.PSObject.Properties[$Name]) { $servers.PSObject.Properties.Remove($Name) }
$servers | Add-Member -NotePropertyName $Name -NotePropertyValue ([pscustomobject]$entry)
New-Item -ItemType Directory -Force (Split-Path $Mcp) | Out-Null
[System.IO.File]::WriteAllText($Mcp, ($doc | ConvertTo-Json -Depth 8), $noBom)

Write-Host "root=$app"
Write-Host "work=$Work"
Write-Host "pid=$($proc.Id)"
Write-Host "pipe=\\.\pipe\shikisha-$($proc.Id)"
Write-Host "token-file=$tokenFile"
if ($Port -gt 0) { Write-Host "board=http://127.0.0.1:$Port" }
if ($Cdp -gt 0) { Write-Host "cdp=http://127.0.0.1:$Cdp" }
Write-Host "mcp-config=$Mcp"
Write-Host "mcp=`"$mcpExe`" --mcp --pid $($proc.Id) --token-file `"$tokenFile`""
