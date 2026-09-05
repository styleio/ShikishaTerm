<#
  Fetch the ConPTY that ships beside the executable.

  Windows' in-box ConPTY still parses the child's VT into a text buffer and
  re-renders it into our pipe. Microsoft ships the newer one -- the one that
  copies the child's bytes through untouched -- as a signed NuGet package, and
  portable-pty prefers a conpty.dll found next to the executable over the one
  in the system. So the whole of this feature is "put two files in the right
  place", and this is the script that does it.

    tools/conpty.ps1              fetch if missing, verify, place
    tools/conpty.ps1 -Require     ...and fail the build if it cannot
    tools/conpty.ps1 -Force       fetch again even if the files are already right
    tools/conpty.ps1 -Arch arm64  the other architecture

  Nothing here trusts the network. The package is checked against the hash in
  packaging/conpty/redist.json before it is opened, each extracted file is
  checked against its own hash, and each one's PE header is checked to be the
  architecture asked for. A file that fails any of those is not written.

  Without -Require a failure is a warning and the build continues: the program
  runs on the in-box ConPTY, more slowly and with sequences missing, and says
  so on its diagnostics screen. Releases pass -Require, because a download that
  quietly lost this is a download that is worse than the last one.
#>
param(
    [ValidateSet('x64', 'arm64')][string]$Arch = 'x64',
    [string]$Dest,
    [switch]$Require,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem

$root = Split-Path -Parent $PSScriptRoot
$pinPath = Join-Path $root 'packaging\conpty\redist.json'
if (-not $Dest) { $Dest = Join-Path $root 'vendor\conpty' }
# The download itself is build output, not source, so it lives with the rest of
# the build output and a clean checkout carries none of it.
$cacheRoot = Join-Path $root 'target\conpty-cache'

function Get-Sha256([string]$Path) {
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Assert-Sha256([string]$Path, [string]$Expected, [string]$Label) {
    $actual = Get-Sha256 $Path
    if ($actual -ne $Expected) {
        throw "$Label does not match the pin. expected $Expected, got $actual"
    }
}

# Read the machine field out of the PE header. A package that quietly served
# the wrong architecture would otherwise fail at CreatePseudoConsole time, on
# someone else's machine, as "the terminal is slow today".
function Assert-PeMachine([string]$Path, [string]$Expected) {
    $want = @{ 'x64' = 0x8664; 'arm64' = 0xAA64 }[$Expected]
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $reader = [System.IO.BinaryReader]::new($stream)
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw "$Path is not a PE file" }
        $stream.Position = 0x3C
        $stream.Position = $reader.ReadUInt32()
        if ($reader.ReadUInt32() -ne 0x00004550) { throw "$Path has no PE signature" }
        $machine = $reader.ReadUInt16()
    }
    finally { $stream.Dispose() }
    if ($machine -ne $want) {
        throw ("$Path is built for machine 0x{0:X4}, not $Expected (0x{1:X4})" -f $machine, $want)
    }
}

# The hash already settles what these bytes are, so a signature that cannot be
# read is not a reason to stop a developer's build. It is a reason to say so:
# the one thing we must never do is re-sign Microsoft's binaries ourselves, and
# a broken signature here would mean the pin was raised without looking.
function Test-MicrosoftSignature([string]$Path) {
    try { $sig = Get-AuthenticodeSignature -LiteralPath $Path } catch { return $false }
    if ($sig.Status -ne 'Valid') { return $false }
    return ($sig.SignerCertificate.Subject -like '*O=Microsoft Corporation*')
}

function Expand-Entry($Archive, [string]$EntryPath, [string]$To) {
    $entry = $Archive.GetEntry($EntryPath)
    if (-not $entry) { throw "the pinned package has no entry $EntryPath" }
    $from = $entry.Open()
    try {
        $out = [System.IO.File]::Open($To, [System.IO.FileMode]::CreateNew,
                                      [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
        try { $from.CopyTo($out) } finally { $out.Dispose() }
    }
    finally { $from.Dispose() }
}

function Fail([string]$Message) {
    if ($Require) { throw $Message }
    Write-Warning $Message
    Write-Warning 'The build continues on the in-box ConPTY. Kitty graphics and Sixel are dropped there, OSC 8 is rewritten, and terminal output is slower.'
    exit 0
}

if (-not (Test-Path $pinPath)) { Fail "the ConPTY pin is missing at $pinPath" }
$pin = Get-Content -LiteralPath $pinPath -Raw | ConvertFrom-Json
if ($pin.schemaVersion -ne 1 -or
    $pin.packageId -ne 'Microsoft.Windows.Console.ConPTY' -or
    $pin.license -ne 'MIT') {
    Fail "the ConPTY pin at $pinPath is not one this script understands"
}

$payload = $pin.architectures.PSObject.Properties[$Arch]
if (-not $payload) { Fail "the ConPTY pin carries no $Arch payload" }
$payload = $payload.Value

$destDll = Join-Path $Dest 'conpty.dll'
$destExe = Join-Path $Dest 'OpenConsole.exe'

# Already right? Then this run costs a hash and nothing else. Dev.cmd calls
# this on every build, so that has to be the common case.
if (-not $Force -and (Test-Path $destDll) -and (Test-Path $destExe)) {
    if ((Get-Sha256 $destDll) -eq $payload.conptyDll.sha256 -and
        (Get-Sha256 $destExe) -eq $payload.openConsoleExe.sha256) {
        Write-Host "ConPTY $($pin.version) ($Arch) is already in place: $Dest"
        exit 0
    }
}

$cacheDir = Join-Path $cacheRoot $pin.version
New-Item -ItemType Directory -Force $cacheDir | Out-Null
$packagePath = Join-Path $cacheDir ([System.IO.Path]::GetFileName(([System.Uri]$pin.nupkg.url).AbsolutePath))

if (Test-Path $packagePath) {
    if ((Get-Sha256 $packagePath) -ne $pin.nupkg.sha256) { Remove-Item -LiteralPath $packagePath -Force }
}

if (-not (Test-Path $packagePath)) {
    $partial = "$packagePath.part"
    try {
        Write-Host "downloading ConPTY $($pin.version) ($Arch)"
        $progress = $ProgressPreference
        $ProgressPreference = 'SilentlyContinue'   # the bar is unreadable in CI logs and slows the transfer
        try { Invoke-WebRequest -Uri $pin.nupkg.url -OutFile $partial -UseBasicParsing }
        finally { $ProgressPreference = $progress }
        Assert-Sha256 $partial $pin.nupkg.sha256 'the downloaded ConPTY package'
        Move-Item -LiteralPath $partial -Destination $packagePath -Force
    }
    catch {
        if (Test-Path $partial) { Remove-Item -LiteralPath $partial -Force }
        Fail "could not fetch the ConPTY redistributable: $($_.Exception.Message)"
    }
}

# Unpack into a staging folder inside the destination, so a half-written pair
# is never what the next build finds beside the executable.
New-Item -ItemType Directory -Force $Dest | Out-Null
$stage = Join-Path $Dest (".stage-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $stage | Out-Null
try {
    $stagedDll = Join-Path $stage 'conpty.dll'
    $stagedExe = Join-Path $stage 'OpenConsole.exe'
    $zip = [System.IO.Compression.ZipFile]::OpenRead($packagePath)
    try {
        Expand-Entry $zip $payload.conptyDll.entryPath $stagedDll
        Expand-Entry $zip $payload.openConsoleExe.entryPath $stagedExe
    }
    finally { $zip.Dispose() }

    Assert-Sha256 $stagedDll $payload.conptyDll.sha256 'conpty.dll'
    Assert-Sha256 $stagedExe $payload.openConsoleExe.sha256 'OpenConsole.exe'
    Assert-PeMachine $stagedDll $Arch
    Assert-PeMachine $stagedExe $Arch

    foreach ($f in @($stagedDll, $stagedExe)) {
        if (-not (Test-MicrosoftSignature $f)) {
            $name = Split-Path $f -Leaf
            if ($Require) { throw "$name is not validly signed by Microsoft; refusing to ship it" }
            Write-Warning "$name did not pass an Authenticode check on this machine (the hash pin still matched)"
        }
    }

    Copy-Item -LiteralPath $stagedDll -Destination $destDll -Force
    Copy-Item -LiteralPath $stagedExe -Destination $destExe -Force
}
finally {
    if (Test-Path $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
}

Write-Host "ConPTY $($pin.version) ($Arch) placed in $Dest"
