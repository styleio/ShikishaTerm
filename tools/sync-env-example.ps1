<#
  .env.example is DERIVED from .env, never written by hand.

  The point is the recovery case: lose .env and the only thing standing between
  you and a working machine is remembering which keys existed. Checking that the
  two agree would still let them drift between checks — deriving one from the
  other means they cannot. Comments and blank lines carry over verbatim, so the
  place to write "where do I get this token" is .env itself, right where you are
  when you fill it in.

  Values never leave .env: only the key names cross over.

    -Verify   report drift and exit 1 instead of writing (for a hook or CI)
#>
param([switch]$Verify)

$root = Split-Path -Parent $PSScriptRoot
$envFile = Join-Path $root '.env'
$example = Join-Path $root '.env.example'

# No .env on this machine (a fresh clone, CI): leave the example alone. Wiping it
# would destroy the one record of what the keys are — the exact loss this exists
# to prevent.
if (-not (Test-Path $envFile)) {
    Write-Host "no .env here - leaving .env.example as it is"
    exit 0
}

# UTF-8 explicitly: this shell reads as the ANSI codepage otherwise, and the
# comments that say where each token comes from are the useful half of the file
$out = foreach ($line in Get-Content $envFile -Encoding UTF8) {
    if ($line -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=') { "$($Matches[1])=" }
    else { $line }        # comments and blank lines, as written
}
$text = ($out -join "`n").TrimEnd() + "`n"

if ($Verify) {
    $have = if (Test-Path $example) { (Get-Content $example -Raw -Encoding UTF8) -replace "`r`n", "`n" } else { '' }
    if ($have -eq $text) { Write-Host ".env.example is in step with .env"; exit 0 }
    Write-Host ".env.example has fallen behind .env" -ForegroundColor Red
    $a = ($text -split "`n" | Where-Object { $_ -match '=$' })
    $b = ($have -split "`n" | Where-Object { $_ -match '=$' })
    foreach ($k in $a) { if ($b -notcontains $k) { Write-Host "  missing from the example: $($k.TrimEnd('='))" } }
    foreach ($k in $b) { if ($a -notcontains $k) { Write-Host "  no longer in .env:        $($k.TrimEnd('='))" } }
    exit 1
}

Set-Content -Path $example -Value $text -Encoding utf8 -NoNewline
Write-Host "wrote .env.example ($(($out | Where-Object { $_ -match '=$' }).Count) keys)"
