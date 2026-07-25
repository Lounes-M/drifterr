# Authenticode-sign one file during Tauri bundling.
#
# Invoked by tauri.bundle.windows.signCommand (see ../src-tauri/windows-sign.conf.json)
# once per artifact — the app .exe and the installer — so users never see the
# SmartScreen "Windows protected your PC" wall, which today hides the install
# button and silently costs a large share of Windows installs.
#
# Reads the certificate from the environment rather than taking it as an argument,
# so the PFX path and password never appear in a build log or process listing:
#   WINDOWS_CERTIFICATE_PATH      path to the .pfx staged by the release workflow
#   WINDOWS_CERTIFICATE_PASSWORD  its password (may be empty)
#   WINDOWS_TIMESTAMP_URL         RFC-3161 timestamp server (optional)
#
# Timestamping matters: without it, signatures stop validating the day the
# certificate expires, retroactively breaking every release already shipped.

param(
    [Parameter(Mandatory = $true)][string]$Path
)

$ErrorActionPreference = 'Stop'

$pfx = $env:WINDOWS_CERTIFICATE_PATH
if ([string]::IsNullOrWhiteSpace($pfx)) {
    throw "WINDOWS_CERTIFICATE_PATH is not set. This script only runs when the release workflow has staged a certificate."
}
if (-not (Test-Path -LiteralPath $pfx)) {
    throw "Certificate not found at '$pfx'."
}

# Prefer the newest signtool.exe available; the Windows SDK installs it under a
# versioned path, and the runner image's PATH does not always include it.
$signtool = (Get-Command signtool.exe -ErrorAction SilentlyContinue)?.Source
if (-not $signtool) {
    $candidates = Get-ChildItem -Path 'C:\Program Files (x86)\Windows Kits\10\bin' `
        -Filter 'signtool.exe' -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\' } |
        Sort-Object FullName -Descending
    if (-not $candidates) { throw "signtool.exe not found. Is the Windows SDK installed on this runner?" }
    $signtool = $candidates[0].FullName
}

$timestampUrl = if ([string]::IsNullOrWhiteSpace($env:WINDOWS_TIMESTAMP_URL)) {
    'http://timestamp.digicert.com'
} else {
    $env:WINDOWS_TIMESTAMP_URL
}

$args = @('sign', '/fd', 'SHA256', '/td', 'SHA256', '/tr', $timestampUrl, '/f', $pfx)
if (-not [string]::IsNullOrEmpty($env:WINDOWS_CERTIFICATE_PASSWORD)) {
    $args += @('/p', $env:WINDOWS_CERTIFICATE_PASSWORD)
}
$args += $Path

Write-Host "Signing $Path"
# Never echo $args — it may carry the certificate password.
& $signtool @args
if ($LASTEXITCODE -ne 0) { throw "signtool failed with exit code $LASTEXITCODE." }

# A signature that doesn't verify is worse than none: it looks tampered with.
& $signtool 'verify' '/pa' '/v' $Path
if ($LASTEXITCODE -ne 0) { throw "Signature verification failed for '$Path'." }
Write-Host "Signed and verified $Path"
