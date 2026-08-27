<#
  Build the Store copy: an MSIX package of the same binary the zip carries.

  Two things travel differently here than in the download. What ships with the
  program and is only ever read -- lang, profiles, the automation manual -- goes
  inside the package, beside the exe, exactly as before. What belongs to the
  person using it does not: an installed package runs from a read-only folder,
  so config, data, logs and workspaces live under LOCALAPPDATA instead. The
  program decides that for itself at run time (see config::root_dir); nothing
  here has to arrange it.

  What goes in is still dist.list's to decide -- tools/stage.ps1 does the
  copying, so the Store copy and the download cannot drift apart.

    tools/msix.ps1                       build an unsigned package
    tools/msix.ps1 -SelfSign             sign it with a local test certificate
    tools/msix.ps1 -SelfSign -Install    ...and install it, for trying it out

  -SelfSign is for trying the package on this machine and nothing else. The
  Store signs the real one: a package submitted there needs no certificate of
  ours at all, which is the reason for going this way in the first place.
#>
param(
    [switch]$SelfSign,
    [switch]$Install,
    [string]$Publisher = 'CN=SHIKISHA-TERM Test',
    [string]$IdentityName = 'SHIKISHATERM.Test',
    # Must match the publisher display name on the Partner Center account
    # exactly, or the Store rejects the submission.
    [string]$PublisherDisplayName = 'WIRED & ECO, K.K.'
)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$out  = Join-Path $root 'target\msix'
$stage = Join-Path $out 'pkg'

# The Windows SDK, wherever it happens to be. Picking the newest rather than a
# pinned version: these tools are backward compatible, and a pinned path breaks
# on the next machine that has a different SDK installed.
$kits = 'C:\Program Files (x86)\Windows Kits\10\bin'
$sdk = Get-ChildItem $kits -Directory -ErrorAction SilentlyContinue |
       Where-Object { Test-Path (Join-Path $_.FullName 'x64\makeappx.exe') } |
       Sort-Object Name -Descending | Select-Object -First 1
if (-not $sdk) { throw "makeappx.exe not found under $kits -- install the Windows SDK" }
$makeappx = Join-Path $sdk.FullName 'x64\makeappx.exe'
$signtool = Join-Path $sdk.FullName 'x64\signtool.exe'

# The version comes from the one place that already carries it. MSIX wants four
# parts and the Store requires the last to be zero.
$manifestToml = Get-Content (Join-Path $root 'Cargo.toml') -Raw
if ($manifestToml -notmatch '(?m)^version = "(?<v>\d+\.\d+\.\d+)"') { throw "no version in Cargo.toml" }
$version = $Matches['v'] + '.0'

$exe = Join-Path $root 'target\release\SHIKISHA-TERM.exe'
if (-not (Test-Path $exe)) { throw "build it first: cargo build --release" }

if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null

# The same payload the download gets, from the same list.
& (Join-Path $PSScriptRoot 'stage.ps1') -Dest $stage -Package -Exe $exe | Out-Null

# ...minus the parts that only make sense outside a package. Settings.cmd
# launches the exe by path, and an installed copy is started from the Start menu.
Remove-Item (Join-Path $stage 'Settings.cmd') -ErrorAction SilentlyContinue

Copy-Item (Join-Path $root 'packaging\msix\Assets') $stage -Recurse -Force

$manifest = Get-Content (Join-Path $root 'packaging\msix\AppxManifest.xml') -Raw
$manifest = $manifest.Replace('{{IDENTITY_NAME}}', $IdentityName).
                      Replace('{{PUBLISHER}}', $Publisher).
                      Replace('{{VERSION}}', $version).
                      Replace('{{PUBLISHER_DISPLAY_NAME}}', $PublisherDisplayName)
[System.IO.File]::WriteAllText((Join-Path $stage 'AppxManifest.xml'), $manifest,
                               [System.Text.UTF8Encoding]::new($false))

$msix = Join-Path $out "SHIKISHA-TERM-$version.msix"
if (Test-Path $msix) { Remove-Item $msix -Force }
& $makeappx pack /d $stage /p $msix /o
if ($LASTEXITCODE -ne 0) { throw "makeappx failed" }
Write-Host "built $msix"

if ($SelfSign) {
    # A certificate for this machine only, whose subject matches the Publisher in
    # the manifest -- Windows compares the two and refuses the install if they
    # differ by so much as a space.
    $cert = Get-ChildItem Cert:\CurrentUser\My |
            Where-Object { $_.Subject -eq $Publisher -and $_.NotAfter -gt (Get-Date) } |
            Select-Object -First 1
    if (-not $cert) {
        $cert = New-SelfSignedCertificate -Type Custom -Subject $Publisher `
            -KeyUsage DigitalSignature -FriendlyName 'SHIKISHA-TERM local test' `
            -CertStoreLocation 'Cert:\CurrentUser\My' `
            -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
        Write-Host "made a test certificate: $($cert.Thumbprint)"
    }
    & $signtool sign /fd SHA256 /sha1 $cert.Thumbprint $msix
    if ($LASTEXITCODE -ne 0) { throw "signtool failed" }

    # Windows trusts nothing self-signed until it is told to. Trusted People is
    # the store sideloading reads, and it is per-user, so this needs no admin.
    $store = [System.Security.Cryptography.X509Certificates.X509Store]::new('TrustedPeople', 'CurrentUser')
    $store.Open('ReadWrite'); $store.Add($cert); $store.Close()
    Write-Host "signed, and the test certificate is trusted for this user"
}

if ($Install) {
    if (-not $SelfSign) { throw "-Install needs -SelfSign: Windows will not install an unsigned package" }
    Add-AppxPackage -Path $msix -ForceUpdateFromAnyVersion
    Write-Host "installed. Start it from the Start menu, or:"
    Write-Host "  explorer.exe shell:AppsFolder\$IdentityName" + "_$((Get-AppxPackage $IdentityName).PublisherId)!SHIKISHATERM"
}
