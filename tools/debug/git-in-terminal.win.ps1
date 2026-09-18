<#
  Whether a git typed in a terminal tab signs in as the account its project
  chose -- checked through the running app, not through the code that decides
  it.

  A copy of this checkout's build is started in a folder of its own, with a
  desk holding one git account (a token), a project that chose it, and a tab
  whose command asks git itself who it would sign in as. What that tab writes
  is then read here. Nothing of a copy somebody is using is read, written or
  stopped, and the token is a made-up string that reaches no server.

    cargo build, then tools\debug\git-in-terminal.win.ps1 [-At <folder>]

    -Exe       the build to run (default: this checkout's target\debug)
    -At        where to put the copy (default: %TEMP%\sk-git-terminal)
    -Keep      leave the copy running afterwards, to look at it
    -NoAccount the other half: with no account chosen, the terminal is left
               exactly as it was, and git there goes on signing in the way
               this machine already does
#>
param(
    [string]$Exe,
    [string]$At = (Join-Path $env:TEMP 'sk-git-terminal'),
    [switch]$Keep,
    [switch]$NoAccount
)
$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $PSScriptRoot          # tools
$root = Split-Path -Parent $here                  # the checkout
$app = Join-Path $At 'app'
$work = Join-Path $At 'work'
if (-not $Exe) { $Exe = Join-Path $root 'target\debug\SHIKISHA-TERM.exe' }
if (-not (Test-Path $Exe)) { throw "no build at $Exe -- run cargo build first" }

# Only the copies under this folder, the same rule instance.win.ps1 follows:
# stopping by name would take a copy somebody is using down with it
function Stop-Copy {
    Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path -like (Join-Path $At '*') } |
        ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }
}

Stop-Copy
Start-Sleep -Milliseconds 500
if (Test-Path $At) { Remove-Item -Recurse -Force $At }
foreach ($d in @($app, $work, (Join-Path $At 'localappdata'))) {
    New-Item -ItemType Directory -Force -Path $d | Out-Null
}

# The payload beside the exe travels by the one copier, as everywhere else
& (Join-Path $here 'stage.ps1') -Dest $app -Package -Exe $Exe | Out-Null
if (-not (Test-Path (Join-Path $app 'SHIKISHA-TERM.exe'))) { throw "staging $app failed" }

# The tab's command: it asks git what it would do, rather than this script
# asking on git's behalf. `git credential fill` prints the name and the
# password git would sign in with, and takes no network to answer
$probe = Join-Path $At 'probe.ps1'
$out = Join-Path $At 'probe-out.txt'
@"
`$lines = @()
Get-ChildItem env: |
    Where-Object { `$_.Name -like 'SHIKISHA_GIT*' -or `$_.Name -like 'GIT_CONFIG*' } |
    Sort-Object Name | ForEach-Object { `$lines += "`$(`$_.Name)=`$(`$_.Value)" }
`$lines += '--- git credential fill: the account s server ---'
`$lines += (@('protocol=https', 'host=github.com', '') | & git credential fill 2>&1)
`$lines += '--- git credential fill: another server ---'
`$lines += (@('protocol=https', 'host=gitlab.example.com', '') | & git credential fill 2>&1)
Set-Content -LiteralPath '$out' -Value `$lines -Encoding UTF8
"@ | Set-Content -LiteralPath $probe -Encoding UTF8

$token = 'github_pat_madeup_0123456789'
$settings = [ordered]@{
    language = 'en'
    desks    = @([ordered]@{
        name          = 'Check'
        id            = 'check'
        git_accounts  = @([ordered]@{ name = 'work'; login = 'octocat'; user_name = 'Alex Doe'; user_email = 'alex@example.com' })
        projects      = @([ordered]@{ name = 'p'; git_account = $(if ($NoAccount) { $null } else { 'work' }) })
        folders       = @([ordered]@{
            cwd     = $work
            project = 'p'
            tabs    = @([ordered]@{
                name    = 'probe'
                id      = 'probe'
                command = @('powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $probe)
            })
        })
    })
}
$noBom = [System.Text.UTF8Encoding]::new($false)
$cfgDir = Join-Path $app 'config'
New-Item -ItemType Directory -Force -Path $cfgDir | Out-Null
[System.IO.File]::WriteAllText((Join-Path $cfgDir 'config.json'), ($settings | ConvertTo-Json -Depth 8), $noBom)
# The token, filed the way the settings screen files it: git/<desk>/<account>
$secrets = [ordered]@{ tokens = [ordered]@{ 'git/check/work' = $token } }
[System.IO.File]::WriteAllText((Join-Path $cfgDir 'secrets.json'), ($secrets | ConvertTo-Json -Depth 5), $noBom)

# Started with an environment of its own, and without this session's agent
# variables -- the same care instance.win.ps1 takes
$aside = @{}
foreach ($e in [System.Environment]::GetEnvironmentVariables().GetEnumerator()) {
    if ([string]$e.Key -match '^(CLAUDE|ANTHROPIC|SHIKISHA|GIT_CONFIG)') { $aside[[string]$e.Key] = [string]$e.Value }
}
$aside['LOCALAPPDATA'] = $env:LOCALAPPDATA
$proc = $null
try {
    foreach ($k in @($aside.Keys)) { Remove-Item -Path "env:$k" -ErrorAction SilentlyContinue }
    $env:LOCALAPPDATA = Join-Path $At 'localappdata'
    $proc = Start-Process -FilePath (Join-Path $app 'SHIKISHA-TERM.exe') -WorkingDirectory $app -PassThru
} finally {
    foreach ($k in @($aside.Keys)) {
        if ([string]::IsNullOrEmpty($aside[$k])) { Remove-Item -Path "env:$k" -ErrorAction SilentlyContinue }
        else { Set-Item -Path "env:$k" -Value $aside[$k] }
    }
}

$waited = 0
while (-not (Test-Path $out) -and $waited -lt 30000) {
    if ($proc.HasExited) { throw "the copy stopped before its tab ran (exit $($proc.ExitCode))" }
    Start-Sleep -Milliseconds 250
    $waited += 250
}
if (-not $Keep) { Stop-Copy }
if (-not (Test-Path $out)) { throw "the tab wrote nothing in 30s -- see $app\logs" }

$said = Get-Content -LiteralPath $out -Raw
Write-Host "--- what the tab's git said ---"
Write-Host $said
$fail = @()
if ($NoAccount) {
    # Nobody chose, so nothing of ours is in that terminal. A shell that could
    # no longer sign in to anything would be a far worse answer than one that
    # signs in the way it always did
    if ($said -match 'SHIKISHA_GIT|GIT_CONFIG_COUNT') { $fail += 'a terminal nobody chose an account for was changed anyway' }
    if ($said -match [regex]::Escape($token)) { $fail += 'a token reached a terminal that chose no account' }
    if ($fail.Count) {
        Write-Host ''
        foreach ($f in $fail) { Write-Host "FAIL: $f" }
        exit 1
    }
    Write-Host ''
    Write-Host 'PASS: with no account chosen, the terminal is left as it was'
    if ($Keep) { Write-Host "left running at $At" }
    return
}
if ($said -notmatch [regex]::Escape("SHIKISHA_GIT_TOKEN=$token")) { $fail += 'the token never reached the terminal' }
if ($said -notmatch 'GIT_CONFIG_VALUE_\d+=credential\.https://github\.com\.helper|credential\.https://github\.com\.helper') {
    $fail += 'the account helper is not set for github.com'
}
if ($said -notmatch "password=$([regex]::Escape($token))") { $fail += 'git would sign in with something else' }
if ($said -notmatch 'username=octocat') { $fail += 'git would sign in as somebody else' }
if ($said -notmatch 'user\.name') { $fail += "the account's name on commits did not reach the terminal" }
# The account is for its own server. Another one is left to whatever this
# machine already does about it, which is the whole reason the helper is set
# under a server rather than over the lot
$elsewhere = ($said -split '--- git credential fill: another server ---')[1]
if ($elsewhere -and $elsewhere -match [regex]::Escape($token)) {
    $fail += "the account's token is handed to other servers as well"
}
if ($fail.Count) {
    Write-Host ''
    foreach ($f in $fail) { Write-Host "FAIL: $f" }
    exit 1
}
Write-Host ''
Write-Host 'PASS: git in that terminal signs in as the account the project chose'
if ($Keep) { Write-Host "left running at $At" }
