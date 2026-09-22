# Every way a split row can be used, against a running copy of the app,
# checking after each press that the things which must always hold still hold.
#
#     powershell -File tools/debug/split-rows.win.ps1
#
# Needs a build (cargo build --bin SHIKISHA-TERM). It lays out a copy of the
# app under D:\ShikishaTerm-split with folders and tabs of its own, drives it
# through the board's own HTTP door, and reads the state back the same way. It
# touches no install anywhere else: every process it stops is matched by FULL
# PATH, because matched by name it once stopped somebody's real work.
#
# Written after a round of "it works" that was one shape and one press at a
# time. Of the bugs it now catches, every one needed either a SEQUENCE --
# divide, go somewhere, come back -- or a shape other than two tabs in one
# folder, and none of them showed up any other way.
#
# Exit code 0 when every check passed; each failure prints the whole state it
# saw, because a check that only says "no" costs another run to find out why.


$ErrorActionPreference = 'Stop'
# The checkout this script is part of, not whichever one somebody happened
# to be in: a tool that tests the build beside it has to say which build
$SRC = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
# Somewhere that is nobody's install. Everything under it is thrown away
# and laid out again for each scenario
$HOME_ = if ($env:SHIKISHA_SPLIT_LAB) { $env:SHIKISHA_SPLIT_LAB }
         elseif (Test-Path 'D:\') { 'D:\ShikishaTerm-split' }
         else { Join-Path $env:TEMP 'ShikishaTerm-split' }
# ...and the working folders the scenarios open, beside it. Forward slashes
# because that is how a settings file spells a path
$WORK = (Join-Path $HOME_ 'work') -replace '\\', '/'
$BASE = 'http://127.0.0.1:8791'
$TOK = 'labtoken0123456789abcdef'
$PORT = 8791

$script:fails = 0
$script:checks = 0
$script:sess = $null

# Only ever the lab's own. Matched by full path: matched by name this once
# caught the copy in C:\google and stopped somebody's real work. A process
# whose path cannot be read is not ours -- reading it can throw, not just
# answer nothing
function Ours {
    foreach ($p in @(Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue)) {
        $path = $null
        try { $path = $p.Path } catch { }
        if ($path -and $path.StartsWith($HOME_, 'OrdinalIgnoreCase')) { $p }
    }
}

function StopLab {
    foreach ($p in @(Ours)) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    # Wait for the file lock to go, not just for the kill to be asked for: the
    # next thing this does is copy a new exe over the one that was running
    for ($i = 0; $i -lt 40; $i++) {
        if (-not (Ours)) {
            try {
                $f = [System.IO.File]::Open("$HOME_\SHIKISHA-TERM.exe", 'Open', 'Write', 'None')
                $f.Close(); return
            } catch { }
        }
        Start-Sleep -Milliseconds 500
    }
}

function Lay([object[]]$folders) {
    # folders: @(@{name='alpha'; cwd="$WORK/alpha"; tabs=2}, ...)
    New-Item -ItemType Directory -Force $HOME_, "$HOME_\config", "$HOME_\logs" | Out-Null
    foreach ($n in 'SHIKISHA-TERM.exe', 'conpty.dll', 'OpenConsole.exe') {
        if (Test-Path "$SRC\target\debug\$n") { Copy-Item "$SRC\target\debug\$n" $HOME_ -Force -ErrorAction SilentlyContinue }
    }
    foreach ($d in 'lang', 'profiles') {
        if (Test-Path "$SRC\$d") { Copy-Item "$SRC\$d" $HOME_ -Recurse -Force -ErrorAction SilentlyContinue }
    }
    $fs = @()
    foreach ($f in $folders) {
        New-Item -ItemType Directory -Force ($f.cwd -replace '/', '\') | Out-Null
        $tabs = @()
        for ($i = 1; $i -le $f.tabs; $i++) {
            $tabs += '{{ "name": "{0}{1}", "id": "{0}-{1}", "command": "cmd" }}' -f $f.name, $i
        }
        $fs += '{{ "name": "{0}", "cwd": "{1}", "tabs": [{2}] }}' -f $f.name, $f.cwd, ($tabs -join ',')
    }
    $json = @"
{
  "language": "en", "resident": false,
  "remote": { "enabled": true, "bind": "127.0.0.1", "port": $PORT, "sticky_token": true, "fixed_token": "$TOK" },
  "desks": [{ "id": "default", "name": "DESK", "folders": [$($fs -join ',')] }]
}
"@
    [System.IO.File]::WriteAllText("$HOME_\config\config.json", $json, (New-Object System.Text.UTF8Encoding($false)))
    Set-Content -Path "$HOME_\logs\hooks.log" -Value '' -Encoding utf8
}

function Start-Lab {
    Start-Process "$HOME_\SHIKISHA-TERM.exe"
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep -Seconds 1
        try {
            $null = Invoke-WebRequest "$BASE/?t=$TOK" -UseBasicParsing -SessionVariable s -TimeoutSec 3
            $script:sess = $s
            Start-Sleep -Seconds 3
            return $true
        } catch { }
    }
    return $false
}

function Say($body) {
    $null = Invoke-WebRequest "$BASE/api/intent?t=$TOK" -Method POST -WebSession $script:sess `
        -Body $body -ContentType 'application/json' -UseBasicParsing -TimeoutSec 8
    Start-Sleep -Seconds 3
}

function State {
    $raw = (Invoke-WebRequest "$BASE/api/state?t=$TOK" -UseBasicParsing -WebSession $script:sess -TimeoutSec 8).Content
    $st = $raw | ConvertFrom-Json
    if ($st.ui) { return $st.ui } else { return $st }
}

function Settings { Get-Content "$HOME_\config\config.json" -Raw | ConvertFrom-Json }

function Ok($cond, $what) {
    $script:checks++
    if ($cond) { return }
    $script:fails++
    "    FAIL  $what"
    # A check that only says "no" costs another run to find out what it saw
    $ui = State
    "          active=$($ui.active) split_open='$($ui.split_open)' board=$($ui.board)"
    foreach ($r in @($ui.tabs)) { "          row #$($r.index) $($r.name) kind=$($r.kind) group=$($r.group) id=$($r.id)" }
    foreach ($f in (Settings).desks[0].folders) {
        "          cfg cwd=$($f.cwd): " + ((@($f.tabs) | ForEach-Object { "$($_.name)[$($_.command)]" }) -join ', ')
    }
}

# Everything that must be true no matter what was just pressed
function Invariants($where) {
    $ui = State
    $cfg = Settings
    $rows = @($ui.tabs)
    $splits = @($rows | Where-Object { $_.kind -eq 'split' })

    # The split in front is a split that exists, and nothing else is in front
    if ($ui.split_open) {
        Ok ($splits | Where-Object { $_.id -eq $ui.split_open }) "$where : split_open '$($ui.split_open)' names no row on the list"
    }

    # Every split row written down still has an arrangement, and it has at
    # least two panes -- a split of one is a row that cannot be pressed
    foreach ($f in $cfg.desks[0].folders) {
        foreach ($tab in @($f.tabs)) {
            if ($tab.command -ne 'split') { continue }
            Ok ($null -ne $tab.panes) "$where : split '$($tab.id)' lost its arrangement"
            if ($tab.panes) {
                Ok (@($tab.panes.keys).Count -ge 2) "$where : split '$($tab.id)' is down to $(@($tab.panes.keys).Count) pane(s)"
            }
        }
    }

    # A split shows rows; it is not one of them
    foreach ($s in $splits) {
        Ok ($null -ne $s.id) "$where : a split row has no name automation can call it by"
    }
    return $ui
}

function SplitsInSettings {
    $out = @()
    foreach ($f in (Settings).desks[0].folders) {
        foreach ($tab in @($f.tabs)) { if ($tab.command -eq 'split') { $out += [pscustomobject]@{ id = $tab.id; cwd = $f.cwd } } }
    }
    return $out
}


# ---- reading what is on screen ------------------------------------------
function RowOf($ui, $name) { (@($ui.tabs) | Where-Object { $_.name -eq $name } | Select-Object -First 1).index }
function SplitRow($ui) { (@($ui.tabs) | Where-Object { $_.kind -eq 'split' } | Select-Object -First 1) }
function NSplits($ui) { @(@($ui.tabs) | Where-Object { $_.kind -eq 'split' }).Count }
function NRows($ui) { @($ui.tabs).Count }
function Held { (@((Settings).desks[0].folders | ForEach-Object { $_.tabs } | Where-Object { $_.command -eq 'split' })[0]).panes.keys }
function HoldsId($id) { @(Held | Where-Object { $_[1] -eq $id }).Count -eq 1 }

# Laid again for every scenario. Sharing one settings file between them left a
# split from the scenario before standing in the next one, and the checks then
# read somebody else's row -- a failure with nothing wrong
function Scenario($name, $folders, $body) {
    "=== $name ==="
    StopLab
    Lay $folders
    if (-not (Start-Lab)) { $script:fails++; "    FAIL  it never came up"; return }
    & $body
    StopLab
}

function Restart {
    StopLab
    if (-not (Start-Lab)) { $script:fails++; "    FAIL  it did not come back up" }
}

# ---- one folder, one tab -------------------------------------------------
Scenario 'one folder, one tab' @(@{name='solo'; cwd="$WORK/solo"; tabs=1}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'solo1'))
    $ui = Invariants 'on solo1'
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'after divide'
    $s = SplitRow $ui
    Ok ($null -ne $s) 'one tab: no split row was made'
    Ok ($ui.split_open -eq $s.id) 'one tab: the split is not the row in front'
    Ok ($s.group -eq 0) 'one tab: the split landed outside the folder'
    # ...and back out again
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'solo1'))
    $ui = Invariants 'back on solo1'
    Ok (-not $ui.split_open) 'one tab: pressing the row did not leave the split'
}

# ---- one folder, three tabs ---------------------------------------------
Scenario 'one folder, three tabs' @(@{name='three'; cwd="$WORK/three"; tabs=3}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'three2'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'after divide on the middle tab'
    $s = SplitRow $ui
    Ok ($ui.split_open -eq $s.id) 'three tabs: the split is not in front'
    # every other row still reachable, and each leaves the split
    foreach ($n in 'three1', 'three2', 'three3') {
        Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui $n))
        $ui = Invariants "on $n"
        Ok (-not $ui.split_open) "three tabs: pressing $n did not leave the split"
        Ok ($ui.active -eq (RowOf $ui $n)) "three tabs: pressing $n did not go there"
    }
    Say ('{{"kind":"select","tab":{0}}}' -f (SplitRow $ui).index)
    $ui = Invariants 'back in the split'
    Ok ($ui.split_open -eq (SplitRow $ui).id) 'three tabs: the split did not come back'
}

# ---- two folders: the sequence that was reported -------------------------
Scenario 'two folders, the reported sequence' @(
    @{name='alpha'; cwd="$WORK/alpha"; tabs=2},
    @{name='beta'; cwd="$WORK/beta"; tabs=2}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'alpha1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'after divide in alpha'
    $s = SplitRow $ui
    Say ('{{"kind":"select","tab":{0}}}' -f $s.index)
    $ui = Invariants 'pressed the split row'
    Ok ($ui.split_open -eq $s.id) 'the split row is not in front after pressing it'
    Say ('{{"kind":"folderview","folder":"{0}"}}' -f "$WORK/beta")
    $ui = Invariants 'pressed the other folder'
    Ok (-not $ui.split_open) 'the split of alpha is still in front while beta is shown'
    Ok (@(@($ui.tabs) | Where-Object { $_.kind -eq 'split' }).Count -eq 1) 'a split appeared somewhere nobody made one'
    $inbeta = @($ui.tabs) | Where-Object { $_.kind -eq 'split' -and $_.group -eq 1 }
    Ok (-not $inbeta) 'a split row turned up in the other folder'
    Ok ((@(SplitsInSettings)).Count -eq 1) 'the settings gained a split nobody made'
    Ok ((@(SplitsInSettings))[0].cwd -eq "$WORK/alpha") 'the split moved folder in the settings'
    # ...and back to alpha's split
    Say ('{{"kind":"folderview","folder":"{0}"}}' -f "$WORK/alpha")
    $ui = Invariants 'back on alpha'
    Say ('{{"kind":"select","tab":{0}}}' -f (SplitRow $ui).index)
    $ui = Invariants 'alpha split again'
    Ok ($ui.split_open -eq (SplitRow $ui).id) 'the split did not come back'
}

# ---- closing and adding tabs while a split holds them --------------------
Scenario 'closing a tab the split is showing' @(
    @{name='alpha'; cwd="$WORK/alpha"; tabs=2},
    @{name='beta'; cwd="$WORK/beta"; tabs=2}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'alpha1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split made'
    $a1 = RowOf $ui 'alpha1'
    Say ('{{"kind":"closetab","tab":{0},"key":"","sure":true}}' -f $a1)
    $ui = Invariants 'alpha1 closed while the split showed it'
    Ok (@(@($ui.tabs) | Where-Object { $_.name -eq 'alpha1' }).Count -eq 0) 'the tab did not close'
    Ok (@(@($ui.tabs) | Where-Object { $_.kind -eq 'split' }).Count -eq 1) 'the split went with the tab it was showing'
    $s = SplitRow $ui
    Say ('{{"kind":"select","tab":{0}}}' -f $s.index)
    $ui = Invariants 'into the split whose tab is gone'
    Ok ($ui.split_open -eq $s.id) 'the split with an empty pane cannot be entered'
}

# ---- three folders, a split in two of them -------------------------------
Scenario 'a split in two folders at once' @(
    @{name='one'; cwd="$WORK/one"; tabs=2},
    @{name='two'; cwd="$WORK/two"; tabs=2},
    @{name='three'; cwd="$WORK/three"; tabs=1}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'one1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split in folder one'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'two1'))
    $ui = Invariants 'moved to folder two'
    Ok (-not $ui.split_open) 'the split of folder one followed into folder two'
    Say '{"kind":"splitpane","id":1,"down":true}'
    $ui = Invariants 'split in folder two as well'
    Ok (@(@($ui.tabs) | Where-Object { $_.kind -eq 'split' }).Count -eq 2) 'the second split was not made'
    $bycwd = @(SplitsInSettings)
    Ok ($bycwd.Count -eq 2) 'the settings do not hold both splits'
    Ok (@($bycwd | Where-Object { $_.cwd -eq "$WORK/one" }).Count -eq 1) 'folder one lost its split'
    Ok (@($bycwd | Where-Object { $_.cwd -eq "$WORK/two" }).Count -eq 1) 'folder two did not get its own'
    # go between them
    $s1 = @($ui.tabs) | Where-Object { $_.kind -eq 'split' -and $_.group -eq 0 } | Select-Object -First 1
    $s2 = @($ui.tabs) | Where-Object { $_.kind -eq 'split' -and $_.group -eq 1 } | Select-Object -First 1
    Say ('{{"kind":"select","tab":{0}}}' -f $s1.index)
    $ui = Invariants 'into the first split'
    Ok ($ui.split_open -eq $s1.id) 'the first split is not in front'
    Say ('{{"kind":"select","tab":{0}}}' -f $s2.index)
    $ui = Invariants 'into the second split'
    Ok ($ui.split_open -eq $s2.id) 'the second split is not in front'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'three1'))
    $ui = Invariants 'into the third folder'
    Ok (-not $ui.split_open) 'a split is still in front in a folder that has none'
}

# ---- the split's own row is closed --------------------------------------
Scenario 'closing the split row itself' @(@{name='a'; cwd="$WORK/a"; tabs=2}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'a1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split made'
    Ok ((NSplits $ui) -eq 1) 'no split was made'
    $s = SplitRow $ui
    Say ('{{"kind":"closetab","tab":{0},"key":"","sure":true}}' -f $s.index)
    $ui = Invariants 'split row closed'
    Ok ((NSplits $ui) -eq 0) 'the split row survived its own close'
    Ok ((RowOf $ui 'a1')) 'a1 went with the split'
    Ok ((RowOf $ui 'a2')) 'a2 went with the split'
    Ok (-not $ui.split_open) 'a closed split is still in front'
    Ok (@(SplitsInSettings).Count -eq 0) 'the settings still hold the closed split'
}

# ---- three panes, then closing them one at a time ------------------------
Scenario 'three panes down to none' @(@{name='a'; cwd="$WORK/a"; tabs=3}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'a1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'two panes'
    Say '{"kind":"splitpane","id":1,"down":true}'
    $ui = Invariants 'three panes'
    Ok ((NSplits $ui) -eq 1) 'dividing a split made a second split row'
    $cfg = @(SplitsInSettings)
    Ok ($cfg.Count -eq 1) 'the settings hold more than one split'
    # close them back down; the row goes when one is left
    Say '{"kind":"closepane","id":3}'
    $ui = Invariants 'back to two panes'
    Ok ((NSplits $ui) -eq 1) 'the split went early'
    Say '{"kind":"closepane","id":2}'
    $ui = Invariants 'down to one'
    Ok ((NSplits $ui) -eq 0) 'a split of one pane is still a row'
    Ok (-not $ui.split_open) 'the closed split is still in front'
    Ok ((RowOf $ui 'a1')) 'a1 was lost with the split'
}

# ---- closing the pane that has something, leaving the empty one ----------
Scenario 'closing the filled pane' @(@{name='a'; cwd="$WORK/a"; tabs=2}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'a1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split made'
    Say '{"kind":"closepane","id":1}'
    $ui = Invariants 'the filled pane closed'
    Ok ((NSplits $ui) -eq 0) 'a split of one empty pane is still a row'
    Ok ((RowOf $ui 'a1')) 'closing a pane ended the tab in it'
    Ok ((RowOf $ui 'a2')) 'a2 disappeared'
}

# ---- a restart, with a split standing -----------------------------------
Scenario 'a restart with a split standing' @(
    @{name='a'; cwd="$WORK/a"; tabs=2},
    @{name='b'; cwd="$WORK/b"; tabs=2}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'a1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split made'
    $before = @(SplitsInSettings).Count
    Restart
    $ui = Invariants 'after the restart'
    Ok ((NSplits $ui) -eq 1) "the split count changed over a restart (rows: $(NSplits $ui))"
    Ok (@(SplitsInSettings).Count -eq $before) 'the settings gained or lost a split over a restart'
    $s = SplitRow $ui
    Ok ($s.group -eq 0) 'the split moved folder over a restart'
    Say ('{{"kind":"select","tab":{0}}}' -f $s.index)
    $ui = Invariants 'into the split after the restart'
    Ok ($ui.split_open -eq $s.id) 'the split cannot be entered after a restart'
    # ...and a second restart must not double anything
    Restart
    $ui = Invariants 'after a second restart'
    Ok ((NSplits $ui) -eq 1) 'a second restart changed the count'
}

# ---- adding a tab while a split is in front -----------------------------
Scenario 'adding a tab with a split in front' @(@{name='a'; cwd="$WORK/a"; tabs=2}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'a1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split made'
    $rows = NRows $ui
    # Pressing the folder you are already in is somebody finding their place,
    # not asking to be moved: nothing should happen at all
    Say ('{{"kind":"folderview","folder":"{0}"}}' -f "$WORK/a")
    $ui = Invariants 'pressed the folder already in front'
    Ok ($ui.split_open) 'pressing the folder it is in threw the split away'
    Ok ((NSplits $ui) -eq 1) 'the folder press changed the split count'
}

# ---- a folder with nothing in it ----------------------------------------
Scenario 'a folder with no tabs beside one with two' @(
    @{name='full'; cwd="$WORK/full"; tabs=2},
    @{name='empty'; cwd="$WORK/empty"; tabs=0}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'full1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split in the full folder'
    Ok ((NSplits $ui) -eq 1) 'no split was made'
    Ok ((SplitRow $ui).group -eq 0) 'the split landed in the empty folder'
    Say ('{{"kind":"folderview","folder":"{0}"}}' -f "$WORK/empty")
    $ui = Invariants 'pressed the empty folder'
    Ok (-not $ui.split_open) 'the split is still in front over an empty folder'
    Ok (@(SplitsInSettings).Count -eq 1) 'the empty folder gained a split'
}

# ---- a tab of ANOTHER folder closed while a split stands ----------------
Scenario 'closing an unrelated tab' @(
    @{name='a'; cwd="$WORK/a"; tabs=2},
    @{name='b'; cwd="$WORK/b"; tabs=2}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'a1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split made in a'
    Say ('{{"kind":"closetab","tab":{0},"key":"","sure":true}}' -f (RowOf $ui 'b2'))
    $ui = Invariants "b2 closed, which the split never showed"
    Ok ((NSplits $ui) -eq 1) 'closing an unrelated tab took the split'
    $s = SplitRow $ui
    Say ('{{"kind":"select","tab":{0}}}' -f $s.index)
    $ui = Invariants 'into the split after the unrelated close'
    Ok ($ui.split_open -eq $s.id) 'the split cannot be entered after an unrelated close'
    $held = (@((Settings).desks[0].folders | ForEach-Object { $_.tabs } | Where-Object { $_.command -eq 'split' })[0]).panes.keys
    Ok (@($held | Where-Object { $_[1] -eq 'a-1' }).Count -eq 1) 'the split forgot the tab it was showing'
}

# ---- fill the empty half from the list, same folder ----------------------
Scenario 'filling the empty half from the same folder' @(@{name='a'; cwd="$WORK/a"; tabs=2}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'a1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split with an empty half'
    Ok ($ui.split_open) 'the split is not in front'
    # the keyboard is in the filled half; move it to the empty one, then press a2
    Say '{"kind":"focuspane","id":2}'
    $ui = Invariants 'keyboard in the empty half'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'a2'))
    $ui = Invariants 'a2 pressed with the empty half in hand'
    Ok ($ui.split_open) 'filling a pane left the split instead'
    Ok (HoldsId 'a-2') 'a2 did not go into the empty half'
    Ok (HoldsId 'a-1') 'a1 fell out of the other half'
}

# ---- fill it with a row of ANOTHER folder -------------------------------
Scenario 'a pane showing another folder' @(
    @{name='a'; cwd="$WORK/a"; tabs=2},
    @{name='b'; cwd="$WORK/b"; tabs=2}) {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'a1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split in a'
    Say '{"kind":"focuspane","id":2}'
    $ui = Invariants 'keyboard in the empty half'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'b1'))
    $ui = Invariants "b1 of the other folder put into the pane"
    Ok ($ui.split_open) 'putting another folder in a pane left the split'
    Ok (HoldsId 'b-1') "the other folder's row is not in the pane"
    Ok (HoldsId 'a-1') 'the first half was lost'
    # the split still belongs to the folder it was made in
    Ok ((SplitRow $ui).group -eq 0) 'the split moved folder because of what it shows'
    Ok (@(SplitsInSettings)[0].cwd -eq "$WORK/a") 'the settings moved the split'
    # leave and come back
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'b2'))
    $ui = Invariants 'away to b2'
    Ok (-not $ui.split_open) 'the split did not let go'
    Say ('{{"kind":"select","tab":{0}}}' -f (SplitRow $ui).index)
    $ui = Invariants 'back into the split'
    Ok ($ui.split_open) 'the split did not come back'
    Ok (HoldsId 'b-1') 'it forgot the other folder it was showing'
    # ...and across a restart
    Restart
    $ui = Invariants 'after a restart'
    Ok ((NSplits $ui) -eq 1) 'the split count changed over a restart'
    Ok (HoldsId 'b-1') 'the other folder was forgotten over a restart'
    Ok (HoldsId 'a-1') 'its own folder was forgotten over a restart'
    # closing the far tab empties that pane and keeps the shape
    $ui = Invariants 'before closing b1'
    Say ('{{"kind":"closetab","tab":{0},"key":"","sure":true}}' -f (RowOf $ui 'b1'))
    $ui = Invariants 'b1 closed while a pane showed it'
    Ok ((NSplits $ui) -eq 1) 'the split went with the far tab'
    Ok (@(Held).Count -eq 2) 'the arrangement lost a pane'
    Ok (HoldsId 'a-1') 'its own half went too'
}

# Two desks, the divider dragged, and the split's ✕ pressed from elsewhere.


function TwoDeskScenario($name, $body) {
    "=== $name ==="
    StopLab
    # Laid again for every scenario. Sharing one settings file between them
    # left a split from the scenario before standing in the next one, and the
    # checks then read somebody else's row -- a failure with nothing wrong
    LayTwoDesks
    if (-not (Start-Lab)) { $script:fails++; "    FAIL  it never came up"; return }
    & $body
    StopLab
}

# Two desks, laid out by hand: the generator makes one
function LayTwoDesks {
    New-Item -ItemType Directory -Force $HOME_, "$HOME_\config", "$HOME_\logs", "$WORK/d1", "$WORK/d2" | Out-Null
    foreach ($n in 'SHIKISHA-TERM.exe', 'conpty.dll', 'OpenConsole.exe') {
        if (Test-Path "$SRC\target\debug\$n") { Copy-Item "$SRC\target\debug\$n" $HOME_ -Force -ErrorAction SilentlyContinue }
    }
    foreach ($d in 'lang', 'profiles') { if (Test-Path "$SRC\$d") { Copy-Item "$SRC\$d" $HOME_ -Recurse -Force -ErrorAction SilentlyContinue } }
    $json = @"
{
  "language": "en", "resident": false,
  "remote": { "enabled": true, "bind": "127.0.0.1", "port": 8791, "sticky_token": true, "fixed_token": "labtoken0123456789abcdef" },
  "desks": [
    { "id": "one", "name": "ONE", "folders": [ { "name": "d1", "cwd": "$WORK/d1", "tabs": [
        { "name": "x1", "id": "x-1", "command": "cmd" }, { "name": "x2", "id": "x-2", "command": "cmd" } ] } ] },
    { "id": "two", "name": "TWO", "folders": [ { "name": "d2", "cwd": "$WORK/d2", "tabs": [
        { "name": "y1", "id": "y-1", "command": "cmd" }, { "name": "y2", "id": "y-2", "command": "cmd" } ] } ] }
  ]
}
"@
    [System.IO.File]::WriteAllText("$HOME_\config\config.json", $json, (New-Object System.Text.UTF8Encoding($false)))
    Set-Content -Path "$HOME_\logs\hooks.log" -Value '' -Encoding utf8
}

TwoDeskScenario 'switching desks with a split standing' {
    $ui = Invariants 'start on desk one'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'x1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split on desk one'
    Ok ((NSplits $ui) -eq 1) 'no split on desk one'
    # The desk list, then its number -- the way the board does it
    Say '{"kind":"opendesk"}'
    Say '{"kind":"key","text":"2"}'
    $ui = Invariants 'moved to desk two'
    Ok (-not $ui.split_open) "desk one's split is still in front on desk two"
    Ok ((NSplits $ui) -eq 0) "desk one's split row is listed on desk two"
    Say '{"kind":"opendesk"}'
    Say '{"kind":"key","text":"1"}'
    $ui = Invariants 'back on desk one'
    Ok ((NSplits $ui) -eq 1) 'the split did not survive the desk switch'
    $s = SplitRow $ui
    Say ('{{"kind":"select","tab":{0}}}' -f $s.index)
    $ui = Invariants 'into it after the switch'
    Ok ($ui.split_open -eq $s.id) 'it cannot be entered after a desk switch'
}

TwoDeskScenario 'the divider is dragged, then left and returned to' {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'x1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split made'
    Say '{"kind":"paneratio","divider":0,"ratio":0.25}'
    $ui = Invariants 'divider dragged'
    $kept = (@((Settings).desks[0].folders | ForEach-Object { $_.tabs } | Where-Object { $_.command -eq 'split' })[0]).panes.layout.root.ratio
    Ok ($kept -and [math]::Abs($kept - 0.25) -lt 0.02) "the dragged ratio was not written down (got '$kept')"
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'x2'))
    $ui = Invariants 'away'
    Say ('{{"kind":"select","tab":{0}}}' -f (SplitRow $ui).index)
    $ui = Invariants 'back'
    $after = (@((Settings).desks[0].folders | ForEach-Object { $_.tabs } | Where-Object { $_.command -eq 'split' })[0]).panes.layout.root.ratio
    Ok ($after -and [math]::Abs($after - 0.25) -lt 0.02) "the ratio was lost on the way back (got '$after')"
}

TwoDeskScenario 'the split row closed from another row' {
    $ui = Invariants 'start'
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'x1'))
    Say '{"kind":"splitpane","id":1,"down":false}'
    $ui = Invariants 'split made'
    $s = SplitRow $ui
    # stand somewhere else, then close the split's row from the list
    Say ('{{"kind":"select","tab":{0}}}' -f (RowOf $ui 'x2'))
    $ui = Invariants 'standing on x2'
    Ok (-not $ui.split_open) 'still in the split while standing on x2'
    Say ('{{"kind":"closetab","tab":{0},"key":"split:{1}","sure":true}}' -f $s.index, $s.id)
    $ui = Invariants 'split row closed from x2'
    Ok ((NSplits $ui) -eq 0) 'the split row survived being closed from elsewhere'
    Ok ((RowOf $ui 'x1')) 'x1 went with it'
    Ok ((RowOf $ui 'x2')) 'x2 went with it'
    Ok (@(SplitsInSettings).Count -eq 0) 'the settings still hold it'
}


"";"checks: $script:checks   failures: $script:fails"
if ($script:fails -gt 0) { exit 1 } else { exit 0 }
