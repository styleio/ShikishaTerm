# Whether a tab on a desk that is not in front can still reach the app with
# its own `shikisha`, against a running copy of the app.
#
#     powershell -File tools/debug/background-desk-key.win.ps1
#
# Needs a build (cargo build --bin SHIKISHA-TERM). It lays out a copy of the
# app under D:\ShikishaTerm-keylab (or %TEMP% without a D: drive) with two
# desks, each with one tab whose program calls `shikisha list` every two
# seconds and writes down whether the app answered. Then it switches to the
# other desk and back through the board's own door, and reads what each tab
# wrote. It touches no install anywhere else: every process it stops is
# matched by FULL PATH.
#
# Checked: a tab's calls are answered while its desk is in front, while
# another desk is in front, and after its desk is in front again.
#
# Exit code 0 when every check passed.

$ErrorActionPreference = 'Stop'
$SRC = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$HOME_ = if ($env:SHIKISHA_KEY_LAB) { $env:SHIKISHA_KEY_LAB }
         elseif (Test-Path 'D:\') { 'D:\ShikishaTerm-keylab' }
         else { Join-Path $env:TEMP 'ShikishaTerm-keylab' }
$WORK = (Join-Path $HOME_ 'work') -replace '\\', '/'
$PORT = 8793
$BASE = "http://127.0.0.1:$PORT"
$TOK = 'keylabtoken0123456789abcdef'
$script:fails = 0

function Ours { Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($HOME_, 'OrdinalIgnoreCase') } }
function StopLab {
    foreach ($p in @(Ours)) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    for ($i = 0; $i -lt 40 -and (Ours); $i++) { Start-Sleep -Milliseconds 500 }
}
function Ok($cond, $what) { if ($cond) { "  PASS $what" } else { $script:fails++; "  FAIL $what" } }

StopLab
if (Test-Path $HOME_) { Remove-Item $HOME_ -Recurse -Force }
New-Item -ItemType Directory -Force $HOME_, "$HOME_\config", "$HOME_\logs", "$WORK/one", "$WORK/two" | Out-Null
foreach ($n in 'SHIKISHA-TERM.exe', 'conpty.dll', 'OpenConsole.exe') {
    if (Test-Path "$SRC\target\debug\$n") { Copy-Item "$SRC\target\debug\$n" $HOME_ -Force }
}
foreach ($d in 'lang', 'profiles') { if (Test-Path "$SRC\$d") { Copy-Item "$SRC\$d" $HOME_ -Recurse -Force } }

# The program each tab runs: `shikisha list`, every two seconds, and a line
# saying whether the app answered
$loop = @'
param($log)
while ($true) {
    $out = (& shikisha list 2>&1 | Out-String)
    $said = if ($LASTEXITCODE -eq 0 -and $out -notmatch 'unauthorized|closed') { 'ok' } else { 'refused ' + ($out -replace '\s+', ' ').Trim() }
    Add-Content -Path $log -Value ("{0} {1}" -f [DateTimeOffset]::Now.ToUnixTimeSeconds(), $said)
    Start-Sleep -Seconds 2
}
'@
Set-Content -Path "$HOME_\loop.ps1" -Value $loop
$cmd = { param($name) "powershell -NoProfile -ExecutionPolicy Bypass -File $($HOME_ -replace '\\', '/')/loop.ps1 $($HOME_ -replace '\\', '/')/$name.log" }
$json = @"
{
  "language": "en", "resident": false,
  "remote": { "enabled": true, "bind": "127.0.0.1", "port": $PORT, "sticky_token": true, "fixed_token": "$TOK" },
  "desks": [
    { "id": "one", "name": "ONE", "folders": [ { "name": "one", "cwd": "$WORK/one", "tabs": [
        { "name": "caller", "id": "caller-one", "command": "$(& $cmd 'one')" } ] } ] },
    { "id": "two", "name": "TWO", "folders": [ { "name": "two", "cwd": "$WORK/two", "tabs": [
        { "name": "caller", "id": "caller-two", "command": "$(& $cmd 'two')" } ] } ] }
  ]
}
"@
Set-Content -Path "$HOME_\config\config.json" -Value $json

Start-Process "$HOME_\SHIKISHA-TERM.exe" -ArgumentList '--behind'
$sess = $null
for ($i = 0; $i -lt 30 -and -not $sess; $i++) {
    Start-Sleep -Seconds 1
    try { $null = Invoke-WebRequest "$BASE/?t=$TOK" -UseBasicParsing -SessionVariable s -TimeoutSec 3; $sess = $s } catch { }
}
if (-not $sess) { "  FAIL it never came up"; StopLab; exit 1 }
function Say($body) {
    $null = Invoke-WebRequest "$BASE/api/intent?t=$TOK" -Method POST -WebSession $sess -Body $body -ContentType 'application/json' -UseBasicParsing -TimeoutSec 8
    Start-Sleep -Seconds 1
}
# What a tab wrote after `since` (seconds since 1970)
function Said($name, $since) {
    if (-not (Test-Path "$HOME_\$name.log")) { return @() }
    @(Get-Content "$HOME_\$name.log" | ForEach-Object { $t, $w = $_ -split ' ', 2; if ([long]$t -ge $since) { $w } })
}
function Now { [DateTimeOffset]::Now.ToUnixTimeSeconds() }

"1. desk ONE in front"
Start-Sleep -Seconds 8
$t = Now; Start-Sleep -Seconds 6
$one = Said 'one' $t
Ok ($one.Count -gt 0 -and @($one | Where-Object { $_ -ne 'ok' }).Count -eq 0) "ONE's tab is answered while its desk is in front: $($one -join ' | ')"

"2. desk TWO in front"
Say '{"kind":"opendesk"}'; Say '{"kind":"key","text":"2"}'
Start-Sleep -Seconds 8
$t = Now; Start-Sleep -Seconds 8
$one = Said 'one' $t; $two = Said 'two' $t
Ok ($two.Count -gt 0 -and @($two | Where-Object { $_ -ne 'ok' }).Count -eq 0) "TWO's tab is answered: $($two -join ' | ')"
Ok ($one.Count -gt 0 -and @($one | Where-Object { $_ -ne 'ok' }).Count -eq 0) "ONE's tab is answered from behind: $($one -join ' | ')"

"3. desk ONE in front again"
Say '{"kind":"opendesk"}'; Say '{"kind":"key","text":"1"}'
Start-Sleep -Seconds 4
$t = Now; Start-Sleep -Seconds 8
$one = Said 'one' $t; $two = Said 'two' $t
Ok ($one.Count -gt 0 -and @($one | Where-Object { $_ -ne 'ok' }).Count -eq 0) "ONE's tab is answered once its desk is back: $($one -join ' | ')"
Ok ($two.Count -gt 0 -and @($two | Where-Object { $_ -ne 'ok' }).Count -eq 0) "TWO's tab is answered from behind: $($two -join ' | ')"

if (-not $env:KEYLAB_KEEP) { StopLab }
if ($script:fails) { "failures: $script:fails"; exit 1 } else { "all passed" }
