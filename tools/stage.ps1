<#
  Put the payload named in dist.list into a folder.

  One copier for every destination there is: the test machine (Deploy.cmd and the
  deploy hook) and the download (release.yml). build.rs reads the same list from
  the Rust side for the dev build. When something new has to travel, it is added
  to dist.list and it reaches all of them at once — the arrangement that made a
  forgotten copy possible is the one this replaces.

    -Dest      where to stage into (created if absent)
    -Package   also carry the [package] section (the download; not the dev machine)
    -Exe       an exe to place at the root of it
#>
param(
    [Parameter(Mandatory)][string]$Dest,
    [switch]$Package,
    [string]$Exe
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$list = Join-Path $root 'dist.list'
if (-not (Test-Path $list)) { throw "dist.list is missing at $list" }

# Read it into sections. Deliberately dumb parsing: this file is read by a Rust
# build script too, and a format that needs a library on both sides is a format
# that will disagree with itself one day.
$section = ''
$want = @{ 'beside-exe' = @(); 'beside-exe-flat' = @(); 'package' = @() }
foreach ($line in Get-Content $list -Encoding UTF8) {
    $t = $line.Trim()
    if ($t -eq '' -or $t.StartsWith('#')) { continue }
    if ($t -match '^\[(.+)\]$') { $section = $Matches[1]; continue }
    if ($want.ContainsKey($section)) { $want[$section] += $t }
}

$sections = @('beside-exe', 'beside-exe-flat')
if ($Package) { $sections += 'package' }

New-Item -ItemType Directory -Force $Dest | Out-Null
if ($Exe) { Copy-Item $Exe (Join-Path $Dest (Split-Path $Exe -Leaf)) -Force }

$staged = 0
foreach ($s in $sections) {
    foreach ($pattern in $want[$s]) {
        $recurse = $pattern.EndsWith('/**')
        $rel = if ($recurse) { $pattern.Substring(0, $pattern.Length - 3) } else { $pattern }
        $src = Join-Path $root ($rel -replace '/', '\')
        # beside-exe-flat drops the folder the file came from: it has to end up
        # next to the executable itself, because that is the only place Windows
        # looks (conpty.dll). Everywhere else keeps the shape it is written in.
        $sub = if ($s -eq 'beside-exe-flat') { '' } else { Split-Path $rel -Parent }
        $to  = if ($sub) { Join-Path $Dest $sub } else { $Dest }

        if ($recurse) {
            if (-not (Test-Path $src)) { Write-Host "  (absent, skipped) $pattern"; continue }
            New-Item -ItemType Directory -Force (Join-Path $Dest $rel) | Out-Null
            Copy-Item "$src\*" (Join-Path $Dest $rel) -Recurse -Force
            $staged++
            continue
        }
        $hits = @(Get-ChildItem $src -File -ErrorAction SilentlyContinue)
        if ($hits.Count -eq 0) { Write-Host "  (nothing matched, skipped) $pattern"; continue }
        New-Item -ItemType Directory -Force $to | Out-Null
        $hits | ForEach-Object { Copy-Item $_.FullName (Join-Path $to $_.Name) -Force }
        $staged += $hits.Count
    }
}
Write-Host "staged $staged item(s) into $Dest"
