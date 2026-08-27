<#
  Write THIRD-PARTY-NOTICES.txt: every license the download carries, in full.

  The permissive licenses this program is built from -- MIT, BSD, Apache,
  ISC -- all grant what they grant on one condition, that their copyright
  notice travels with the code. Handing out a binary that reproduces none of
  them is handing it out outside the grant. So the notices ship in the zip,
  and this is the one command that writes them.

  What goes in and where its text is found is about.toml's to decide; the
  shape of the file is about.hbs's. Neither is guesswork -- a crate whose
  license cannot be identified stops the run rather than being credited to
  nobody.

    tools/notices.ps1           rewrite THIRD-PARTY-NOTICES.txt
    tools/notices.ps1 -Check    do not write; fail if the committed file is stale

  CI runs -Check, so a dependency added without rerunning this is caught at
  the pull request rather than inside a release someone already downloaded.
#>
param([switch]$Check)
$ErrorActionPreference = 'Stop'

# Pinned on purpose. -Check compares generated text against a committed file,
# so the generator has to be the same one everywhere or CI fails over a
# difference nobody made.
$version = '0.9.2'

$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root 'THIRD-PARTY-NOTICES.txt'

# cargo is not always on PATH here (it is installed per-user, and a shell that
# did not read the profile will not see it), so fall back to where rustup puts it.
$cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (-not $cargo) { $cargo = Join-Path $HOME '.cargo/bin/cargo.exe' }
if (-not (Test-Path $cargo)) { throw "cargo not found; install rustup first" }

# cargo-about only builds its command line under the `cli` feature. Without it
# the install reports success and installs no binary at all, which reads as
# "installed, but broken" the next time around.
$about = Join-Path (Split-Path -Parent $cargo) 'cargo-about.exe'
$have = if (Test-Path $about) { (& $about --version) -replace '^cargo-about\s+', '' } else { '' }
if ($have.Trim() -ne $version) {
    Write-Host "installing cargo-about $version"
    & $cargo install cargo-about --locked --features cli --version $version
    if ($LASTEXITCODE -ne 0) { throw "could not install cargo-about $version" }
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "notices-$PID.txt"
try {
    & $cargo about generate --locked `
        -c (Join-Path $root 'about.toml') (Join-Path $root 'about.hbs') -o $tmp
    if ($LASTEXITCODE -ne 0) { throw "cargo about could not account for every license" }

    # Read and write UTF-8 explicitly. Windows PowerShell reads an unmarked file
    # in the machine's ANSI code page, which on a Japanese Windows turns the (c)
    # in Lua's notice into something else and reports a difference nobody made;
    # and its -Encoding utf8 writes a byte order mark this file has no use for.
    $utf8 = [System.Text.UTF8Encoding]::new($false)

    # Line endings are git's business, not this file's: the working copy is CRLF
    # here and LF on a build machine, and comparing those raw would report a
    # difference on every line of a file nobody touched.
    $fresh = [System.IO.File]::ReadAllText($tmp, $utf8) -replace "`r`n", "`n"
    $known = if (Test-Path $dest) { [System.IO.File]::ReadAllText($dest, $utf8) -replace "`r`n", "`n" } else { '' }

    if ($Check) {
        if ($fresh -eq $known) {
            Write-Host "THIRD-PARTY-NOTICES.txt is current"
            exit 0
        }
        Write-Host "THIRD-PARTY-NOTICES.txt is out of date -- run tools/notices.ps1 and commit it"

        # Say what differs, not merely that something does. A check that fails on
        # a build machine and reports nothing leaves the difference to be guessed
        # at from a machine that cannot reproduce it.
        $a = $known -split "`n"
        $b = $fresh -split "`n"
        Write-Host "  committed: $($a.Count) lines, $($known.Length) chars"
        Write-Host "  generated: $($b.Count) lines, $($fresh.Length) chars"

        $was = @($a | Select-String -Pattern '^  \* (\S+ \S+)$' | ForEach-Object { $_.Matches[0].Groups[1].Value })
        $now = @($b | Select-String -Pattern '^  \* (\S+ \S+)$' | ForEach-Object { $_.Matches[0].Groups[1].Value })
        foreach ($c in ($now | Where-Object { $was -notcontains $_ })) { Write-Host "  + $c" }
        foreach ($c in ($was | Where-Object { $now -notcontains $_ })) { Write-Host "  - $c" }

        # The crate lists can agree while the license text moves, so show the
        # first place the two actually part company.
        for ($i = 0; $i -lt [Math]::Max($a.Count, $b.Count); $i++) {
            if ($a[$i] -ne $b[$i]) {
                Write-Host "  first difference at line $($i + 1):"
                Write-Host "    committed: $($a[$i])"
                Write-Host "    generated: $($b[$i])"
                break
            }
        }
        exit 1
    }

    if ($fresh -eq $known) {
        Write-Host "THIRD-PARTY-NOTICES.txt unchanged"
        exit 0
    }
    [System.IO.File]::WriteAllText($dest, $fresh, $utf8)

    # Distinct crates, not lines: one crate under two licenses is listed under
    # both, and reporting 262 where 210 were credited invites the wrong question.
    $n = ([regex]::Matches($fresh, '(?m)^  \* (\S+ \S+)$') | ForEach-Object { $_.Groups[1].Value } |
          Sort-Object -Unique).Count
    Write-Host "wrote THIRD-PARTY-NOTICES.txt ($n crates)"
} finally {
    Remove-Item $tmp -ErrorAction SilentlyContinue
}
