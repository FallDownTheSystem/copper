<#
.SYNOPSIS
    Exercise the capture cascade end to end against Notepad, without risking the
    caller's clipboard.

.DESCRIPTION
    The cascade's clipboard fallback replaces and then restores the clipboard.
    If the restore path is broken, whatever was on the clipboard is gone — so
    this script saves the caller's clipboard text first, puts a known sentinel
    there instead, and puts the original back at the end regardless of outcome.
    That means even a total failure of the spike's restore is recoverable.

    Two things are checked:
      * that the cascade captured the text selected in Notepad, and
      * that the sentinel came back afterwards, which is the restore working.

.PARAMETER NoSelection
    Skip the Ctrl+A, so the "nothing selected" case runs instead. This is the
    case acceptance criterion 9 is about — record what happens to the target.

.EXAMPLE
    pwsh -File spike\scripts\verify-cascade.ps1
    pwsh -File spike\scripts\verify-cascade.ps1 -NoSelection
#>
[CmdletBinding()]
param(
    [switch]$NoSelection,
    [int]$UiaTimeoutMs = 250,
    [int]$ClipboardTimeoutMs = 200
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms

Write-Host 'This script takes over the foreground window and injects Ctrl+A and Ctrl+C.' -ForegroundColor Yellow
Write-Host 'Do not run it while you are using the machine.' -ForegroundColor Yellow

# Windows' focus-stealing prevention can refuse AppActivate, in which case the
# capture would run against whatever the user actually had in front — measuring
# the wrong application and injecting Ctrl+C into it. So the target is verified
# rather than assumed.
Add-Type -Namespace CopperSpike -Name Fg -MemberDefinition @'
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
'@

function Get-ForegroundPid {
    $hwnd = [CopperSpike.Fg]::GetForegroundWindow()
    $procId = 0
    [void][CopperSpike.Fg]::GetWindowThreadProcessId($hwnd, [ref]$procId)
    return $procId
}

function Wait-ForForeground([int]$TargetPid, [int]$TimeoutMs = 5000) {
    $shell = New-Object -ComObject WScript.Shell
    $deadline = (Get-Date).AddMilliseconds($TimeoutMs)
    while ((Get-Date) -lt $deadline) {
        if ((Get-ForegroundPid) -eq $TargetPid) { return $true }
        $null = $shell.AppActivate($TargetPid)
        Start-Sleep -Milliseconds 250
    }
    return (Get-ForegroundPid) -eq $TargetPid
}

$spikeRoot = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $spikeRoot 'capture\target\debug\examples\cascade-selftest.exe'
if (-not (Test-Path $exe)) {
    throw "Build it first: cargo build --manifest-path $spikeRoot\capture\Cargo.toml --example cascade-selftest"
}

# --- Save whatever the caller had on the clipboard --------------------------
$savedClipboard = $null
try { $savedClipboard = Get-Clipboard -Raw -ErrorAction Stop } catch { }
if ($savedClipboard) {
    Write-Host "Saved $($savedClipboard.Length) characters of your clipboard; it will be put back."
} else {
    Write-Host 'Your clipboard held no text (or could not be read as text).'
}

$sentinel = "COPPER-CLIPBOARD-SENTINEL-$([guid]::NewGuid())"
$sampleText = 'The quick brown fox jumps over the lazy dog.'
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) 'copper-cascade-selftest.txt'
Set-Content -Path $tmp -Value $sampleText -Encoding UTF8 -NoNewline

$notepad = $null
$outFile = Join-Path ([System.IO.Path]::GetTempPath()) 'copper-selftest-out.json'

try {
    Set-Clipboard -Value $sentinel
    Start-Sleep -Milliseconds 200

    $notepad = Start-Process notepad -ArgumentList $tmp -PassThru
    Start-Sleep -Milliseconds 1500

    if (-not (Wait-ForForeground -TargetPid $notepad.Id)) {
        throw ("Notepad would not come to the foreground (Windows focus-stealing prevention). " +
               "Aborting rather than injecting Ctrl+C into whatever is actually in front. " +
               "Close other windows, click the desktop, and try again.")
    }

    # Start the capture on a delay, then re-assert Notepad's focus and select,
    # so Notepad is definitely the foreground window when the cascade runs.
    $proc = Start-Process -FilePath $exe `
        -ArgumentList '--delay-ms', '2500', '--uia-timeout-ms', $UiaTimeoutMs, '--clipboard-timeout-ms', $ClipboardTimeoutMs `
        -RedirectStandardOutput $outFile -PassThru -WindowStyle Hidden

    Start-Sleep -Milliseconds 400
    if (-not (Wait-ForForeground -TargetPid $notepad.Id -TimeoutMs 1200)) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        throw 'Notepad lost focus after the capture was scheduled. Aborting.'
    }

    if (-not $NoSelection) {
        [System.Windows.Forms.SendKeys]::SendWait('^a')
        Start-Sleep -Milliseconds 300
    }

    if (-not $proc.WaitForExit(30000)) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        throw 'cascade-selftest did not finish within 30 s'
    }

    $json = Get-Content $outFile -Raw
    Write-Host "`n--- cascade result ---"
    Write-Host $json

    $result = $json | ConvertFrom-Json
    $after = Get-Clipboard -Raw

    Write-Host "`n--- checks ---"
    $captured = if ($NoSelection) { '(no selection expected)' } else { $result.preview }
    Write-Host ("outcome              : {0}" -f $result.outcome)
    Write-Host ("strategy             : {0}" -f $result.strategy)
    Write-Host ("captured             : {0}" -f $captured)
    Write-Host ("restore stage        : {0}" -f $result.stages.restore)

    if ($after -eq $sentinel) {
        Write-Host 'CLIPBOARD RESTORE    : PASS - the sentinel came back intact' -ForegroundColor Green
    } else {
        Write-Host 'CLIPBOARD RESTORE    : FAIL - the sentinel did NOT come back' -ForegroundColor Red
        Write-Host ("  clipboard now holds: {0}" -f $after)
    }

    if (-not $NoSelection) {
        if ($result.preview -eq $sampleText) {
            Write-Host 'CAPTURED TEXT        : PASS - matches the Notepad selection' -ForegroundColor Green
        } else {
            Write-Host 'CAPTURED TEXT        : FAIL - does not match the Notepad selection' -ForegroundColor Red
        }
    }
}
finally {
    if ($notepad -and -not $notepad.HasExited) {
        Stop-Process -Id $notepad.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item $tmp -ErrorAction SilentlyContinue
    Remove-Item $outFile -ErrorAction SilentlyContinue

    if ($savedClipboard) {
        Set-Clipboard -Value $savedClipboard
        Write-Host "`nYour original clipboard has been put back."
    } else {
        # Set-Clipboard -Value '' throws on Windows PowerShell 5.1, so clear it
        # through the .NET API instead.
        [System.Windows.Forms.Clipboard]::Clear()
        Write-Host "`nClipboard cleared (it held no text when this started)."
    }
}
