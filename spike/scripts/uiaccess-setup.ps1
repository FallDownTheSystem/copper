<#
.SYNOPSIS
    Generate a self-signed code-signing certificate, sign the uiAccess test
    binary, install it under %ProgramFiles%, and report what to do next.
    Acceptance criterion 12 of task-001.

.DESCRIPTION
    Windows grants the UIAccess token flag only when all three of the following
    hold, and this script exists to satisfy the first two:

      1. the executable is Authenticode-signed by a certificate present in both
         the machine's Trusted Root Certification Authorities and its Trusted
         Publishers stores,
      2. it lives in a secure location - %ProgramFiles%, %ProgramFiles(x86)% or
         %SystemRoot%\System32, including subdirectories, and
      3. UAC is enabled.

    This is the procedure dsgn-001 Phase 4 will use for the release build, so
    every step is scripted rather than described. MUST BE RUN ELEVATED: writing
    to LocalMachine certificate stores and to %ProgramFiles% both require admin.

    Note that the *validation run* afterwards must NOT be elevated. uiAccess
    exists so the process does not need elevation; running the test elevated
    would grant high integrity for an unrelated reason and prove nothing.

.PARAMETER Subject
    Certificate subject. Defaults to a Copper-specific name so it is easy to
    find and remove later.

.PARAMETER Remove
    Undo everything: delete the certificates from both stores and remove the
    installed directory.

.EXAMPLE
    # In an elevated PowerShell:
    pwsh -File spike\scripts\uiaccess-setup.ps1

    # Then, in a NORMAL (unelevated) shell, with an elevated Windows Terminal
    # in the foreground and text selected:
    & "$env:ProgramFiles\copper-test\uiaccess-test.exe"

.EXAMPLE
    pwsh -File spike\scripts\uiaccess-setup.ps1 -Remove
#>
[CmdletBinding()]
param(
    [string]$Subject = 'CN=Copper Spike uiAccess Test',
    [string]$InstallDir = (Join-Path $env:ProgramFiles 'copper-test'),
    [switch]$Remove
)

$ErrorActionPreference = 'Stop'

function Assert-Elevated {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'This script must be run from an ELEVATED PowerShell (certificate stores and %ProgramFiles% both need admin).'
    }
}

function Get-SignTool {
    $candidates = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin' -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\' } |
        Sort-Object FullName -Descending
    if (-not $candidates) {
        throw 'signtool.exe not found. Install the Windows SDK (Windows App Certification Kit component).'
    }
    return $candidates[0].FullName
}

Assert-Elevated

# --------------------------------------------------------------------------
if ($Remove) {
    Write-Host 'Removing the uiAccess test certificate and installation...'
    foreach ($store in @('Cert:\LocalMachine\Root', 'Cert:\LocalMachine\TrustedPublisher', 'Cert:\LocalMachine\My')) {
        Get-ChildItem $store -ErrorAction SilentlyContinue |
            Where-Object { $_.Subject -eq $Subject } |
            ForEach-Object {
                Write-Host "  removing $($_.Thumbprint) from $store"
                Remove-Item $_.PSPath -Force
            }
    }
    if (Test-Path $InstallDir) {
        Remove-Item $InstallDir -Recurse -Force
        Write-Host "  removed $InstallDir"
    }
    Write-Host 'Done.'
    return
}

# --------------------------------------------------------------------------
# 1. Build the test binary.
$spikeRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $spikeRoot 'uiaccess-test\Cargo.toml'
Write-Host '1. Building uiaccess-test (release)...'
& cargo build --release --manifest-path $manifestPath
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
$builtExe = Join-Path $spikeRoot 'uiaccess-test\target\release\uiaccess-test.exe'
if (-not (Test-Path $builtExe)) { throw "expected $builtExe" }

# 2. Generate (or reuse) the code-signing certificate.
Write-Host "`n2. Certificate..."
$cert = Get-ChildItem Cert:\LocalMachine\My | Where-Object { $_.Subject -eq $Subject } | Select-Object -First 1
if ($cert) {
    Write-Host "   reusing existing certificate $($cert.Thumbprint)"
} else {
    $cert = New-SelfSignedCertificate `
        -Type CodeSigningCert `
        -Subject $Subject `
        -CertStoreLocation Cert:\LocalMachine\My `
        -KeyUsage DigitalSignature `
        -KeyExportPolicy Exportable `
        -NotAfter (Get-Date).AddYears(5) `
        -HashAlgorithm SHA256
    Write-Host "   created $($cert.Thumbprint)"
}

# 3. Trust it, in BOTH stores. Root alone is not enough - Windows checks
#    Trusted Publishers for the uiAccess decision specifically.
Write-Host "`n3. Installing into Trusted Root and Trusted Publishers..."
$tmpCer = Join-Path ([System.IO.Path]::GetTempPath()) 'copper-uiaccess.cer'
Export-Certificate -Cert $cert -FilePath $tmpCer -Force | Out-Null
foreach ($store in @('Root', 'TrustedPublisher')) {
    Import-Certificate -FilePath $tmpCer -CertStoreLocation "Cert:\LocalMachine\$store" | Out-Null
    Write-Host "   installed into LocalMachine\$store"
}
Remove-Item $tmpCer -Force -ErrorAction SilentlyContinue

# 4. Install to a secure location, then sign IN PLACE.
#    Order matters: signing then copying is fine, but signing in place makes it
#    obvious that the file which runs is the file that was signed.
Write-Host "`n4. Installing to $InstallDir..."
New-Item -ItemType Directory -Force $InstallDir | Out-Null
$installedExe = Join-Path $InstallDir 'uiaccess-test.exe'
Copy-Item $builtExe $installedExe -Force

Write-Host "`n5. Signing..."
$signtool = Get-SignTool
& $signtool sign /fd SHA256 /sha1 $cert.Thumbprint /tr http://timestamp.digicert.com /td SHA256 $installedExe
if ($LASTEXITCODE -ne 0) {
    Write-Warning 'Timestamping failed (no network?). Retrying without a timestamp - fine for a local test, not for release.'
    & $signtool sign /fd SHA256 /sha1 $cert.Thumbprint $installedExe
    if ($LASTEXITCODE -ne 0) { throw 'signtool failed' }
}

Write-Host "`n6. Verifying the signature..."
& $signtool verify /pa /v $installedExe

Write-Host "`n=== Done. ===" -ForegroundColor Green
Write-Host @"

NEXT STEP - and it must be done from a NORMAL, UNELEVATED shell:

  1. Open Windows Terminal AS ADMINISTRATOR and type some text into it.
  2. In a separate, ordinary (unelevated) shell, run:

       & "$installedExe"

  3. Select text in the elevated terminal during the countdown.

The tool prints its own token state first. What you are looking for is
'token UIAccess : YES' together with 'token elevated : no'. A read that
succeeds while elevated proves nothing about uiAccess.

To undo all of this:
  pwsh -File "$PSCommandPath" -Remove
"@
