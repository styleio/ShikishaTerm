# Checks the private restart script's actual process selector against supplied
# records. Never runs the script body, lists real processes, stops anything,
# or launches the app. PowerShell 5.1 and newer.
#   powershell -NoProfile -File tools/debug/restart-selection.win.ps1
param([string]$Script = (Join-Path $PSScriptRoot '../../.private/Restart-Install.ps1'))
$ErrorActionPreference = 'Stop'
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    (Resolve-Path -LiteralPath $Script).Path, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw "The restart script does not parse: $errors" }
$selector = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
    $node.Name -eq 'Get-RunningInstall'
}, $true)
if (-not $selector) { throw 'The restart selector is missing' }
# Load only this inspected function; none of the script's top-level actions.
Invoke-Expression $selector.Extent.Text
function Get-CimInstance { $script:records }
function Get-Process { throw 'The selector must examine each process role' }
function Stop-Process { throw 'A selector must not stop anything' }
function Start-Process { throw 'A selector must not launch anything' }
$install = 'C:\isolated-restart-check'
$app = "$install\SHIKISHA-TERM.exe"
$records = @(
    [pscustomobject]@{ ProcessId=1; ExecutablePath=$app; CommandLine="`"$app`"" },
    [pscustomobject]@{ ProcessId=2; ExecutablePath=$app; CommandLine="`"$app`" --keeper C:\own-keeper" },
    [pscustomobject]@{ ProcessId=3; ExecutablePath="$app.old-20261003"; CommandLine="`"$app`" --behind" },
    [pscustomobject]@{ ProcessId=4; ExecutablePath=$app; CommandLine="`"$app`" --keeper-launch C:\own-keeper" },
    [pscustomobject]@{ ProcessId=5; ExecutablePath=$app; CommandLine="`"$app`" --cli tab_list" },
    [pscustomobject]@{ ProcessId=6; ExecutablePath=$app; CommandLine="`"$app`" --mcp" },
    [pscustomobject]@{ ProcessId=7; ExecutablePath='C:\another-copy\SHIKISHA-TERM.exe'; CommandLine='SHIKISHA-TERM.exe' },
    [pscustomobject]@{ ProcessId=8; ExecutablePath=$app; CommandLine=$null },
    [pscustomobject]@{ ProcessId=9; ExecutablePath=$null; CommandLine='SHIKISHA-TERM.exe' },
    [pscustomobject]@{ ProcessId=10; ExecutablePath="$app.old"; CommandLine="`"$app`" `"--keeper`" C:\own-keeper" }
)
$selected = @(Get-RunningInstall | ForEach-Object { $_.Id })
if (($selected -join ',') -ne '1,3') { throw "Wrong restart targets: $selected" }
Write-Output 'PASS: only the installed window processes are selected; resident and helper processes survive.'
