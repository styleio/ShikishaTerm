<#
  Write THIRD-PARTY-NOTICES.txt: every license the download carries, in full.

  The permissive licenses this program is built from -- MIT, BSD, Apache,
  ISC -- all grant what they grant on one condition, that their copyright
  notice travels with the code. Handing out a binary that reproduces none of
  them is handing it out outside the grant. So the notices ship in the zip,
  and this is the one command that writes them.

  What may go in, and where each license's text is found, is about.toml's to
  decide, and none of it is guesswork: a crate whose license cannot be
  identified stops the run rather than being credited to nobody.

    tools/notices.ps1           rewrite THIRD-PARTY-NOTICES.txt
    tools/notices.ps1 -Check    do not write; fail if the committed file is stale

  CI runs -Check, so a dependency added without rerunning this is caught at the
  pull request rather than inside a release someone already downloaded. That
  check compares text, so the text has to come out the same on every machine --
  which is why the laying out happens here and not in the template. See about.hbs.
#>
param([switch]$Check)
$ErrorActionPreference = 'Stop'

# Pinned on purpose. -Check compares generated text against a committed file, so
# the generator has to be the same one everywhere or CI fails over a difference
# nobody made.
$version = '0.9.2'

$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root 'THIRD-PARTY-NOTICES.txt'

$header = @'
SHIKISHA-TERM -- third-party notices
====================================

SHIKISHA-TERM itself is under the MIT License; see LICENSE.

It is built from the open source components listed below. Each of them is
distributed under its own terms, and every one of those terms is reproduced
here in full, because that is the condition on which they were given. The list
is worked out from Cargo.lock for the Windows build, so it describes the exact
versions this program is made of -- not a general list of things the project
has used.

Rebuild it with tools/notices.ps1.
'@

# The program is not a third party to itself. cargo-about counts the crate it
# was pointed at like any other, which put SHIKISHA-TERM in its own notices
# under a canonical MIT text reading "Copyright (c) <year> <copyright holders>"
# -- crediting this program to nobody, a few lines below a header that names its
# license properly. Read the name from the manifest so a rename cannot quietly
# undo this.
$manifest = Get-Content (Join-Path $root 'Cargo.toml') -Raw
if ($manifest -notmatch '(?m)^name = "(?<n>[^"]+)"') { throw "no package name in Cargo.toml" }
$self = $Matches['n']

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

$utf8 = [System.Text.UTF8Encoding]::new($false)

# Sort by the bytes, never by the machine's language.
#
# PowerShell's Sort-Object compares the way the current culture reads, and
# cultures disagree about punctuation: under ja-JP "lua-src" sorts after
# "luajit-src" (the hyphen carries no weight, so it reads as luasrc vs
# luajitsrc), under en-US it sorts before. That put two crates in a different
# order on a build machine than on the machine that wrote the file, and -Check
# failed over a difference nobody had made -- in a file whose whole purpose is
# to be the same everywhere.
function Sort-Ordinal($items, [scriptblock]$keyOf) {
    $vals = [object[]]@($items)
    if ($vals.Count -lt 2) { return $vals }
    $keys = [string[]]@($vals | ForEach-Object { & $keyOf $_ })
    [Array]::Sort($keys, $vals, [System.StringComparer]::Ordinal)
    return $vals
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "notices-$PID.txt"
try {
    & $cargo about generate --locked `
        -c (Join-Path $root 'about.toml') (Join-Path $root 'about.hbs') -o $tmp
    if ($LASTEXITCODE -ne 0) { throw "cargo about could not account for every license" }

    # Read UTF-8 explicitly: Windows PowerShell reads an unmarked file in the
    # machine's ANSI code page, which on a Japanese Windows turns the (c) in
    # Lua's notice into something else.
    #
    # Every carriage return goes, not just the ones paired with a newline: 168
    # lines across these licenses end in a bare CR, and a bare CR does not
    # survive being written out and read back the same way twice -- so the file
    # was different from itself on the second run, and no comparison could ever
    # settle. Invisible whitespace is not part of anybody's license terms.
    $raw = [System.IO.File]::ReadAllText($tmp, $utf8) -replace "`r", ""

    # --- Read the tagged form back -------------------------------------------
    $sections = [System.Collections.Generic.List[object]]::new()
    $cur = $null
    $mode = ''
    $text = [System.Text.StringBuilder]::new()
    foreach ($line in ($raw -split "`n")) {
        if ($line.StartsWith('@@LICENSE@@')) {
            $cur = [pscustomobject]@{
                Name   = $line.Substring(11)
                Crates = [System.Collections.Generic.List[object]]::new()
                Text   = ''
            }
            $mode = 'crates'
            continue
        }
        if ($line -eq '@@TEXT@@') { $mode = 'text'; [void]$text.Clear(); continue }
        if ($line -eq '@@END@@') {
            # The blank line before the marker is the template's, not the license's.
            $cur.Text = $text.ToString().TrimEnd("`n")
            $sections.Add($cur)
            $mode = ''
            continue
        }
        if ($mode -eq 'crates' -and $line.StartsWith('@@CRATE@@')) {
            $f = $line.Substring(9).Split('|')
            $cur.Crates.Add([pscustomobject]@{ Name = $f[0]; Version = $f[1]; Repo = $f[2] })
            continue
        }
        if ($mode -eq 'text') { [void]$text.AppendLine($line) }
    }
    if ($sections.Count -eq 0) { throw "cargo about produced no licenses at all" }

    # --- Put it in an order that is the same on every machine ------------------
    $ordered = [System.Collections.Generic.List[object]]::new()
    foreach ($s in $sections) {
        $seen = [System.Collections.Generic.HashSet[string]]::new()
        $keep = [System.Collections.Generic.List[object]]::new()
        foreach ($c in $s.Crates) {
            if ($c.Name -eq $self) { continue }
            if ($seen.Add($c.Name + ' ' + $c.Version)) { $keep.Add($c) }
        }
        if ($keep.Count -eq 0) { continue }   # a section that was only ever us
        $keep = Sort-Ordinal $keep { param($c) $c.Name + "`u{1}" + $c.Version }
        $ordered.Add([pscustomobject]@{
            Name   = $s.Name
            Text   = $s.Text
            Crates = $keep
            # Two crates can be under the same license and different copies of its
            # text -- 128 of these sections are called "MIT License" -- so the name
            # alone cannot order them. What orders them is who they apply to.
            # ...and the text last, so that two sections naming the same license
            # for the same crates still have an order. Nothing is left for the
            # sort to decide: Array.Sort is free to place equal keys either way.
            Key    = $s.Name + "`u{1}" + (($keep | ForEach-Object { $_.Name + ' ' + $_.Version }) -join ',') +
                     "`u{1}" + $s.Text
        })
    }

    $out = [System.Text.StringBuilder]::new()
    [void]$out.AppendLine($header)
    foreach ($s in (Sort-Ordinal $ordered { param($x) $x.Key })) {
        if ($s.Text -match '(?m)^@@') {
            throw "the text of $($s.Name) contains a marker this script parses by"
        }
        [void]$out.AppendLine('')
        [void]$out.AppendLine(('=' * 80))
        [void]$out.AppendLine($s.Name)
        [void]$out.AppendLine(('=' * 80))
        [void]$out.AppendLine('')
        [void]$out.AppendLine('Applies to:')
        foreach ($c in $s.Crates) {
            [void]$out.AppendLine('  * ' + $c.Name + ' ' + $c.Version)
            if ($c.Repo) { [void]$out.AppendLine('    ' + $c.Repo) }
        }
        [void]$out.AppendLine('')
        [void]$out.AppendLine(('-' * 80))
        [void]$out.AppendLine('')
        [void]$out.AppendLine($s.Text)
    }

    # AppendLine writes the platform's line ending, and git hands the committed
    # file back as CRLF on Windows and LF elsewhere. Settle on one, and compare
    # on one, or the check reports a difference on every line of a file nobody
    # touched.
    $fresh = $out.ToString() -replace "`r", ""
    $known = if (Test-Path $dest) { [System.IO.File]::ReadAllText($dest, $utf8) -replace "`r", "" } else { '' }

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
    # both, and reporting the larger number invites the wrong question.
    $names = [System.Collections.Generic.HashSet[string]]::new()
    foreach ($m in [regex]::Matches($fresh, '(?m)^  \* (\S+ \S+)$')) { [void]$names.Add($m.Groups[1].Value) }
    $n = $names.Count
    Write-Host "wrote THIRD-PARTY-NOTICES.txt ($n crates)"
} finally {
    Remove-Item $tmp -ErrorAction SilentlyContinue
}
