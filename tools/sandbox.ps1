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
    tools/sandbox.ps1 -Msix <new> -From <old> -WithRuntime
                                         install the old one, let it settle in,
                                         then upgrade to the new one over it
    tools/sandbox.ps1 -App <folder> -Keep leave the sandbox open to look at

  -App and -Msix are the two ways this program reaches anyone -- the download
  and the Store -- so both can be tried the same way. -App wants a folder
  staged by tools/stage.ps1, which is what the zip is made of.

  A NEW INSTALL AND AN UPGRADE ARE DIFFERENT ROADS. A new install arrives on
  bare ground; an upgrade arrives on top of what the last version left behind,
  which for an installed package is everything under LOCALAPPDATA\SHIKISHA-TERM
  -- config, workspaces, the last session. A version that cannot read what the
  one before it wrote takes away the settings of everyone who updates, and no
  amount of testing a clean install will ever show it. -From is that road.

  What -From does NOT test is the Store carrying the update to anyone. Nothing
  here talks to the Store, to Partner Center or to an account: the packages are
  signed with our own test certificate and put in by hand, inside a machine that
  is destroyed afterwards. Acquisition -- sign in, buy, licence, download -- is
  the one road that needs a real Windows and a real account.

  -WithRuntime installs the WebView2 runtime inside the sandbox first, from
  Microsoft's own address. Without it no version of this program can start
  there, so nothing can write the state an upgrade is supposed to inherit. Leave
  it off to test what a bare machine does; turn it on to test the program.

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
    # An earlier package to install first, so -Msix arrives as an upgrade.
    [string]$From,
    # Install the WebView2 runtime in the sandbox, so the program can start.
    [switch]$WithRuntime,
    # Leave the sandbox running after the report is written.
    [switch]$Keep,
    # Close a sandbox that is already running, rather than refusing to start.
    [switch]$Replace,
    # How long to wait for done.txt, in seconds.
    [int]$Timeout = 300,
    # How many past runs to keep on disk.
    [int]$KeepRuns = 5
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
if ($From) {
    $From = (Resolve-Path $From).Path
    if (-not $Msix) { throw "-From is the version to upgrade FROM; -Msix is the one to upgrade to" }
}

# The shared folder. Under LOCALAPPDATA for the same reason the installed
# program keeps its own things there: it belongs to the person, not the build.
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$runs = Join-Path $env:LOCALAPPDATA 'SHIKISHA-TERM\sandbox'
$work = Join-Path $runs $stamp
New-Item -ItemType Directory -Force -Path $work | Out-Null

# Each run carries a copy of whatever was tested, so this grows by ten megabytes
# a go and nobody ever comes back to look at the twentieth one. Keep the recent
# few and let the rest go.
Get-ChildItem $runs -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending | Select-Object -Skip $KeepRuns |
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
if ($Msix) { Copy-Item $Msix (Join-Path $work 'package.msix') }
if ($From) { Copy-Item $From (Join-Path $work 'previous.msix') }
if ($App) { Copy-Item $App (Join-Path $work 'app') -Recurse }
if ($WithRuntime) { 'runtime' | Set-Content (Join-Path $work 'runtime.txt') }

# ---------------------------------------------------------------- inside ---
# Everything below runs on the other machine. It must never throw: a script
# that dies before writing done.txt leaves this side waiting for the timeout
# with nothing to read, which is the least useful way to fail.

$inside = @'
$ErrorActionPreference = 'Continue'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$log  = Join-Path $here 'log.txt'

# A dead man's handle. Only one sandbox can exist at a time, so a run that
# hangs in here does not merely fail -- it holds the machine, and every run
# after it waits out its own timeout against a machine that will never be
# free. Schedule the end before doing anything that could not come back.
shutdown /s /t 1800 2>&1 | Out-Null
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

    # Can this machine open a web page when asked? Anything that offers a link
    # as the answer to a problem is relying on it, and a machine can have a
    # browser installed and still have nothing registered to open http with.
    # Only in probe mode: it would put a browser in front of the app otherwise.
    if (-not (Test-Path (Join-Path $here 'package.msix')) -and -not (Test-Path (Join-Path $here 'app'))) {
        try {
            Start-Process 'https://example.com' -ErrorAction Stop
            Start-Sleep -Seconds 10
            $r.can_open_page = @(Get-Process msedge -ErrorAction SilentlyContinue).Count -gt 0
            Get-Process msedge -ErrorAction SilentlyContinue | Stop-Process -Force
        } catch { $r.can_open_page = "no: $_" }
        Say "can open a page: $($r.can_open_page)"
    }
}
catch { Say "probe failed: $_" }

# ----------------------------------------------------------- the runtime ---
# Microsoft's own bootstrapper, from Microsoft's own address. Nothing here is
# ours to sign or to host; this is the same download the dialog points a person
# at, taken automatically because there is nobody in here to click it.

if (Test-Path (Join-Path $here 'runtime.txt')) {
    try {
        $setup = Join-Path $env:TEMP 'MicrosoftEdgeWebview2Setup.exe'
        # With a limit on it. A download with no timeout does not fail when the
        # network sulks, it waits -- and this side has nobody to notice, so the
        # whole run is spent sitting on one request that was never coming back.
        Invoke-WebRequest 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $setup -UseBasicParsing -TimeoutSec 120
        Say 'installing the WebView2 runtime'
        Start-Process $setup -ArgumentList '/silent', '/install' -Wait
        $wv2 = (Get-ItemProperty "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" -ErrorAction SilentlyContinue).pv
        $r.runtime_installed = if ($wv2) { $wv2 } else { 'FAILED' }
        Say "runtime now: $($r.runtime_installed)"
    }
    catch { $r.runtime_installed = "failed: $_"; Say "runtime install failed: $_" }
}

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
$prev = Join-Path $here 'previous.msix'

# What an installed package leaves behind between versions. An upgrade that
# cannot read this is an upgrade that empties everyone's settings.
#
# Two places, because a packaged program does not necessarily write where it
# thinks it does: MSIX redirects what an app puts under LOCALAPPDATA into the
# package's own corner, and which of the two it lands in is not ours to decide.
# Looking in only one of them is how a survey comes back empty from a machine
# that has plenty.
function Survey($when) {
    $roots = @(Join-Path $env:LOCALAPPDATA 'SHIKISHA-TERM')
    $roots += @(Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Packages') -Directory -Filter '*SHIKISHA*' -ErrorAction SilentlyContinue |
        ForEach-Object { Join-Path $_.FullName 'LocalCache\Local\SHIKISHA-TERM' })
    $files = @()
    foreach ($root in $roots) {
        if (-not (Test-Path $root)) { continue }
        $files += @(Get-ChildItem $root -Recurse -File -ErrorAction SilentlyContinue |
            ForEach-Object { "$($_.FullName.Substring($env:LOCALAPPDATA.Length)) ($($_.Length))" })
    }
    @{ when = $when; files = @($files | Sort-Object) }
}
# Ask it to close, do not shoot it. What a version writes on its way out --
# last-session above all, which carries a format version and is read by the
# version that comes next -- is exactly the state an upgrade has to inherit,
# and killing the process means testing the upgrade against a machine where
# the previous version never finished a sentence.
function StopIt {
    foreach ($p in @(Get-Process SHIKISHA-TERM -ErrorAction SilentlyContinue)) {
        $null = $p.CloseMainWindow()
    }
    Start-Sleep -Seconds 10
    Get-Process SHIKISHA-TERM -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Seconds 3
}
function LaunchIt($app) {
    $manifest = [xml](Get-Content (Join-Path $app.InstallLocation 'AppxManifest.xml'))
    $aumid = "$($app.PackageFamilyName)!$($manifest.Package.Applications.Application.Id)"
    Start-Process explorer.exe "shell:AppsFolder\$aumid"
    $aumid
}
function TheApp { Get-AppxPackage | Where-Object { $_.Name -like '*SHIKISHA*' } | Select-Object -First 1 }

if (Test-Path $pkg) {
    try {
        # Trust the signer. Taking the certificate out of the package itself
        # means this side needs nothing from the machine that built it.
        foreach ($p in @($prev, $pkg)) {
            if (-not (Test-Path $p)) { continue }
            $sig = Get-AuthenticodeSignature $p
            if ($sig.SignerCertificate) {
                $cer = Join-Path $here "signer-$([IO.Path]::GetFileNameWithoutExtension($p)).cer"
                [IO.File]::WriteAllBytes($cer, $sig.SignerCertificate.Export('Cert'))
                Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
                Say "trusted $($sig.SignerCertificate.Subject) (from $(Split-Path $p -Leaf))"
            } else {
                Say "$(Split-Path $p -Leaf) carries no signature -- Windows will refuse it"
            }
        }

        # The version before, first: let it install, start, and write down
        # whatever it writes down, so the upgrade has something to inherit.
        if (Test-Path $prev) {
            Add-AppxPackage -Path $prev -ErrorAction Stop
            $was = TheApp
            Say "installed the earlier version: $($was.Version)"
            LaunchIt $was | Out-Null
            Start-Sleep -Seconds 30
            StopIt
            $r.before = Survey 'after the earlier version ran'
            Say "the earlier version left $($r.before.files.Count) file(s) behind"
        }

        Add-AppxPackage -Path $pkg -ErrorAction Stop
        $app = TheApp
        if (Test-Path $prev) {
            $r.upgraded = [ordered]@{
                from = $was.Version
                to   = $app.Version
                # Windows refuses an upgrade that does not go up.
                ok   = ([version]$app.Version -gt [version]$was.Version)
            }
            $r.after = Survey 'after the upgrade'
            $lost = @($r.before.files | Where-Object { $_ -notin $r.after.files })
            $r.upgraded.lost = $lost
            Say "upgrade $($was.Version) -> $($app.Version); $($lost.Count) file(s) gone"
        }
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
        Say "launching $(LaunchIt $app)"

        # Long enough for a cold WebView2 to finish its first paint.
        Start-Sleep -Seconds 25

        $r.running = @(Get-Process -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.StartsWith($app.InstallLocation) } |
            ForEach-Object { $_.ProcessName }) | Sort-Object -Unique
        Say "running: $($r.running -join ', ')"

        # And what the new version made of what it inherited. A settings file
        # it could not read is a settings file it would have replaced.
        if (Test-Path $prev) {
            $r.after_running = Survey 'after the new version ran'
            Say "state now: $($r.after_running.files.Count) file(s)"
        }
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

# A dialog that offers a page is only worth anything if the page opens, and
# that is not something the message text can be read to prove. Press OK.
#
# Only a real dialog, though. A message box is window class #32770 and the
# program's own window is not, which is the only way to tell them apart that
# does not depend on what language the title happens to be in -- and pressing
# Enter into a running terminal because its title also said SHIKISHA is a way
# to make a test that types into the thing it is watching.
Add-Type @"
using System;using System.Text;using System.Runtime.InteropServices;
public class Win {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)]
  public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  public static string ClassOf(IntPtr h) { var s = new StringBuilder(64); GetClassName(h, s, 64); return s.ToString(); }
}
"@
try {
    $dlg = Get-Process -ErrorAction SilentlyContinue |
        Where-Object { $_.MainWindowTitle -like '*SHIKISHA*' -and [Win]::ClassOf($_.MainWindowHandle) -eq '#32770' } |
        Select-Object -First 1
    if ($dlg) {
        Add-Type -AssemblyName Microsoft.VisualBasic
        [Microsoft.VisualBasic.Interaction]::AppActivate($dlg.Id)
        Start-Sleep -Seconds 1
        [Windows.Forms.SendKeys]::SendWait('{ENTER}')
        Say 'answered the dialog with OK'
        Start-Sleep -Seconds 15
        $r.page_opened = @(Get-Process msedge -ErrorAction SilentlyContinue).Count -gt 0
        Say "page opened: $($r.page_opened)"

        $b2 = [Windows.Forms.Screen]::PrimaryScreen.Bounds
        $bmp2 = New-Object Drawing.Bitmap $b2.Width, $b2.Height
        $g2 = [Drawing.Graphics]::FromImage($bmp2)
        $g2.CopyFromScreen($b2.Location, [Drawing.Point]::Empty, $b2.Size)
        $bmp2.Save((Join-Path $here 'screen2.png'), [Drawing.Imaging.ImageFormat]::Png)
        $g2.Dispose(); $bmp2.Dispose()
    }
} catch { Say "answering the dialog failed: $_" }

$r | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $here 'report.json') -Encoding UTF8
'done' | Set-Content (Join-Path $here 'done.txt')

shutdown /a 2>&1 | Out-Null
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

# Windows allows exactly one sandbox. Starting a second does not queue, it
# arrives at nothing -- and this side then spends its whole timeout waiting for
# a report from a machine that was never built. Say so instead.
$busy = Get-Process -Name 'WindowsSandboxServer' -ErrorAction SilentlyContinue
if ($busy) {
    if (-not $Replace) {
        throw @"
A sandbox is already running, and Windows only allows one. Close it, or pass
-Replace to have this close it -- its contents are destroyed either way, which
is what a sandbox is for.
"@
    }
    Write-Host 'closing the sandbox that was already running...'
    # Ask the window to close, the way a person would. Shooting the server
    # leaves the virtual machine behind with nobody to shut it down, and a
    # wedged one holds the single slot against every run after it -- which is a
    # worse state than the one being cleaned up.
    Get-Process -Name 'WindowsSandboxClient' -ErrorAction SilentlyContinue |
        ForEach-Object { $null = $_.CloseMainWindow() }

    # vmmemWindowsSandbox is the machine's memory, owned by the system and not
    # ours to stop -- asking is an access denied. It goes when the machine goes.
    $gone = $false
    for ($i = 0; $i -lt 40; $i++) {
        if (-not (Get-Process -Name 'vmmemWindowsSandbox' -ErrorAction SilentlyContinue)) { $gone = $true; break }
        Start-Sleep -Seconds 1
    }
    if (-not $gone) {
        throw @"
The sandbox would not close, and its virtual machine is still holding the one
slot Windows allows. Clear it with an elevated:

  Restart-Service vmcompute -Force
"@
    }
    Start-Sleep -Seconds 3
}

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
