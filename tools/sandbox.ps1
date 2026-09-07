<#
  Try the package on a machine that has never seen this project.

  The problem this solves is that the development machine is the worst
  possible place to test an installer. Five years of tooling is on it: the
  WebView2 runtime, the VC++ redistributables, fonts, a certificate store we
  have been adding to. Every one of those is a thing the package might be
  quietly relying on and never declaring, and none of them can be taken away
  to find out.

  Windows Sandbox is a copy of this Windows with none of that. It boots in
  seconds, the account inside it is already an administrator -- so the test
  certificate goes into the trusted root without a prompt, and without
  staying there afterwards -- and closing the window destroys the whole thing.
  That is the machine we want, and it costs nothing to make a new one.

  What this does NOT test is the Store: there is no Store app inside the
  sandbox, no account to sign in with, and no license to acquire. This checks
  that the PACKAGE installs and runs on a bare machine. Acquisition and update
  are a different question and need a real virtual machine.

    tools/sandbox.ps1                    probe a bare machine and report
    tools/sandbox.ps1 -App <folder>      ...then run the unzipped copy in it
    tools/sandbox.ps1 -Msix <path>       ...or install the packaged one
    tools/sandbox.ps1 -App <folder> -Keep leave the sandbox open to look at

  -App and -Msix are the two ways this program reaches anyone -- the download
  and the Store -- so both can be tried the same way. -App wants a folder
  staged by tools/stage.ps1, which is what the zip is made of.

  The sandbox has no way to talk back to us, so it writes instead: one folder
  is shared read-write, the script inside puts report.json, log.txt and
  screen.png there, and touches done.txt last. This script waits for that file
  and reads what the other machine left behind.
#>
param(
    # A folder staged by tools/stage.ps1 -- the zip, unpacked. Omit to only probe.
    [string]$App,
    # A package built by tools/msix.ps1 -SelfSign. Omit to only probe.
    [string]$Msix,
    # Leave the sandbox running after the report is written.
    [switch]$Keep,
    # How long to wait for done.txt, in seconds.
    [int]$Timeout = 300
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path 'C:\Windows\System32\WindowsSandbox.exe')) {
    throw @"
Windows Sandbox is not installed. Enable it once, in an elevated shell, and
restart the machine:

  Enable-WindowsOptionalFeature -Online -FeatureName Containers-DisposableClientVM -All

It needs Windows Pro, Enterprise or Education. Home cannot run it.
"@
}

if ($Msix) { $Msix = (Resolve-Path $Msix).Path }
if ($App) { $App = (Resolve-Path $App).Path }

# The shared folder. Under LOCALAPPDATA for the same reason the installed
# program keeps its own things there: it belongs to the person, not the build.
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$work = Join-Path $env:LOCALAPPDATA "SHIKISHA-TERM\sandbox\$stamp"
New-Item -ItemType Directory -Force -Path $work | Out-Null
if ($Msix) { Copy-Item $Msix (Join-Path $work 'package.msix') }
if ($App) { Copy-Item $App (Join-Path $work 'app') -Recurse }

# ---------------------------------------------------------------- inside ---
# Everything below runs on the other machine. It must never throw: a script
# that dies before writing done.txt leaves this side waiting for the timeout
# with nothing to read, which is the least useful way to fail.

$inside = @'
$ErrorActionPreference = 'Continue'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$log  = Join-Path $here 'log.txt'
function Say($m) { "$([DateTime]::Now.ToString('HH:mm:ss')) $m" | Tee-Object -FilePath $log -Append }

$r = [ordered]@{}

try {
    Say 'probing a bare machine'

    $os = Get-CimInstance Win32_OperatingSystem
    $r.os = [ordered]@{
        caption = $os.Caption
        build   = "$($os.Version).$((Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').UBR)"
        edition = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').EditionID
    }

    # The WebView2 Evergreen Runtime. This is the one that matters: the window
    # is a WebView2, and if the runtime is absent on a fresh Windows then the
    # program has a dependency it never declared.
    $wvGuid = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    $wv = $null
    foreach ($root in @(
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$wvGuid",
        "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$wvGuid",
        "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$wvGuid"
    )) {
        $v = (Get-ItemProperty $root -ErrorAction SilentlyContinue).pv
        if ($v) { $wv = [ordered]@{ version = $v; where = $root }; break }
    }
    $r.webview2 = if ($wv) { $wv } else { 'ABSENT' }

    # The registry key can be missing on a machine that still has the files, so
    # look for the runtime itself as well before believing the absence.
    $wvDirs = @()
    foreach ($p in @(
        'C:\Program Files (x86)\Microsoft\EdgeWebView\Application',
        'C:\Program Files\Microsoft\EdgeWebView\Application'
    )) {
        if (Test-Path $p) { $wvDirs += (Get-ChildItem $p -Directory -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName }) }
    }
    $r.webview2_files = if ($wvDirs) { $wvDirs } else { 'ABSENT' }
    Say "webview2: key=$($r.webview2 | ConvertTo-Json -Compress) files=$($r.webview2_files | ConvertTo-Json -Compress)"

    # Edge itself is not a substitute: the loader will fall back to a Beta, Dev
    # or Canary channel, but never to stable.
    $r.edge = [ordered]@{}
    foreach ($c in 'Edge', 'Edge Beta', 'Edge Dev', 'Edge SxS') {
        $p = "C:\Program Files (x86)\Microsoft\$c\Application\msedge.exe"
        if (Test-Path $p) { $r.edge[$c] = (Get-Item $p).VersionInfo.ProductVersion }
    }

    # The C runtime the Rust binary links against dynamically.
    $r.vcruntime = if (Test-Path 'C:\Windows\System32\vcruntime140.dll') {
        (Get-Item 'C:\Windows\System32\vcruntime140.dll').VersionInfo.ProductVersion
    } else { 'ABSENT' }

    # ConPTY lives in kernel32 from 1809 (build 17763) on. Below that the
    # bundled conpty.dll is not an optimisation, it is the only way to run.
    $r.conpty_in_os = ([int]((Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').CurrentBuild) -ge 17763)

    $r.internet = try { (Invoke-WebRequest 'http://www.msftconnecttest.com/connecttest.txt' -UseBasicParsing -TimeoutSec 10).StatusCode -eq 200 } catch { $false }
}
catch { Say "probe failed: $_" }

# --------------------------------------------------------- the unzipped copy --

$appDir = Join-Path $here 'app'
if (Test-Path $appDir) {
    try {
        $exe = Get-ChildItem $appDir -Filter *.exe |
            Where-Object { $_.Name -notmatch 'OpenConsole|probe' } | Select-Object -First 1
        Say "running $($exe.Name) from the unzipped copy"
        $p = Start-Process $exe.FullName -WorkingDirectory $appDir -PassThru

        # Long enough for a cold WebView2 to finish its first paint.
        Start-Sleep -Seconds 25

        $p.Refresh()
        $r.app = [ordered]@{
            exe     = $exe.Name
            alive   = -not $p.HasExited
            exit    = if ($p.HasExited) { $p.ExitCode } else { $null }
            # A window with a title is the only proof the WebView came up.
            windows = @(Get-Process -ErrorAction SilentlyContinue |
                Where-Object { $_.MainWindowTitle } |
                ForEach-Object { "$($_.ProcessName): $($_.MainWindowTitle)" })
            children = @(Get-Process -ErrorAction SilentlyContinue |
                Where-Object { $_.Path -and $_.Path.StartsWith($appDir) } |
                ForEach-Object { $_.ProcessName } | Sort-Object -Unique)
        }
        Say "alive=$($r.app.alive) exit=$($r.app.exit) windows=$($r.app.windows -join ' | ')"
    }
    catch {
        $r.app = [ordered]@{ ok = $false; error = "$_" }
        Say "running the unzipped copy failed: $_"
    }
}

# ------------------------------------------------------------- the package --

$pkg = Join-Path $here 'package.msix'
if (Test-Path $pkg) {
    try {
        # Trust the signer. Taking the certificate out of the package itself
        # means this side needs nothing from the machine that built it.
        $sig = Get-AuthenticodeSignature $pkg
        if ($sig.SignerCertificate) {
            $cer = Join-Path $here 'signer.cer'
            [IO.File]::WriteAllBytes($cer, $sig.SignerCertificate.Export('Cert'))
            Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
            Say "trusted $($sig.SignerCertificate.Subject)"
        } else {
            Say 'package carries no signature -- Windows will refuse it'
        }

        Add-AppxPackage -Path $pkg -ErrorAction Stop
        $app = Get-AppxPackage | Where-Object { $_.Name -like '*SHIKISHA*' } | Select-Object -First 1
        $r.install = [ordered]@{
            ok       = $true
            name     = $app.Name
            version  = $app.Version
            location = $app.InstallLocation
        }
        Say "installed $($app.Name) $($app.Version)"

        # Did the bundled console actually travel inside the package?
        $r.install.bundled_conpty = @(
            Get-ChildItem $app.InstallLocation -Recurse -Include conpty.dll, OpenConsole.exe -ErrorAction SilentlyContinue |
                ForEach-Object { $_.FullName.Substring($app.InstallLocation.Length) }
        )

        # Launch it the way the shell does. A packaged app has no exe path to
        # run; it has an identity, and explorer is what resolves one.
        $manifest = [xml](Get-Content (Join-Path $app.InstallLocation 'AppxManifest.xml'))
        $appId = $manifest.Package.Applications.Application.Id
        $aumid = "$($app.PackageFamilyName)!$appId"
        Say "launching $aumid"
        Start-Process explorer.exe "shell:AppsFolder\$aumid"

        # Long enough for a cold WebView2 to finish its first paint.
        Start-Sleep -Seconds 25

        $r.running = @(Get-Process -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.StartsWith($app.InstallLocation) } |
            ForEach-Object { $_.ProcessName }) | Sort-Object -Unique
        Say "running: $($r.running -join ', ')"
    }
    catch {
        $r.install = [ordered]@{ ok = $false; error = "$_" }
        Say "install failed: $_"
    }
} elseif (-not (Test-Path $appDir)) {
    Say 'nothing to install or run -- probe only'
    Start-Sleep -Seconds 3
}

# ------------------------------------------------------------ what it looks like --
try {
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    $b = [Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object Drawing.Bitmap $b.Width, $b.Height
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Location, [Drawing.Point]::Empty, $b.Size)
    $bmp.Save((Join-Path $here 'screen.png'), [Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    Say 'photographed the screen'
} catch { Say "screenshot failed: $_" }

$r | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $here 'report.json') -Encoding UTF8
'done' | Set-Content (Join-Path $here 'done.txt')

if (-not (Test-Path (Join-Path $here 'keep.txt'))) {
    Start-Sleep -Seconds 2
    shutdown /s /t 0
}
'@

Set-Content -Path (Join-Path $work 'inside.ps1') -Value $inside -Encoding UTF8
if ($Keep) { 'keep' | Set-Content (Join-Path $work 'keep.txt') }

# ------------------------------------------------------------------ host ---

$wsb = @"
<Configuration>
  <VGpu>Enable</VGpu>
  <Networking>Enable</Networking>
  <MemoryInMB>4096</MemoryInMB>
  <MappedFolders>
    <MappedFolder>
      <HostFolder>$work</HostFolder>
      <SandboxFolder>C:\work</SandboxFolder>
      <ReadOnly>false</ReadOnly>
    </MappedFolder>
  </MappedFolders>
  <LogonCommand>
    <Command>powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\work\inside.ps1</Command>
  </LogonCommand>
</Configuration>
"@
$wsbPath = Join-Path $work 'run.wsb'
Set-Content -Path $wsbPath -Value $wsb -Encoding UTF8

Write-Host "work folder: $work"
Write-Host 'starting a machine that has never seen this project...'
Start-Process 'C:\Windows\System32\WindowsSandbox.exe' $wsbPath

$done = Join-Path $work 'done.txt'
$deadline = (Get-Date).AddSeconds($Timeout)
while (-not (Test-Path $done) -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 2 }

if (-not (Test-Path $done)) {
    Write-Warning "nothing came back within ${Timeout}s. What it managed to write:"
    Get-Content (Join-Path $work 'log.txt') -ErrorAction SilentlyContinue
    exit 1
}

Write-Host ''
Get-Content (Join-Path $work 'log.txt')
Write-Host ''
Get-Content (Join-Path $work 'report.json')
Write-Host ''
Write-Host "screenshot: $(Join-Path $work 'screen.png')"
