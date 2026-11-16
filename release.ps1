# Builds tonearm.exe and tonearm-setup.exe on this computer, the way the
# Release workflow does, so the installer can be checked before tagging.
#
#   .\release.ps1 0.6.0              build the exe and the installer
#   .\release.ps1 0.6.0 -SkipBuild   only redo the installer from the last exe
#   .\release.ps1 0.6.0 -Publish     also make the GitHub release (needs gh)
#
# The service keys come from keys.env (see build.rs). NSIS comes from
# `winget install NSIS.NSIS`.
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$SkipBuild,
    [switch]$Publish
)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$Version = $Version.TrimStart('v')
$tag = "v$Version"

if ($Publish) {
    if (git status --porcelain) {
        throw "There are uncommitted changes. A release has to match a commit, so commit them first."
    }
}

if (-not $SkipBuild) {
    if (-not (Test-Path keys.env) -and -not $env:TONEARM_LASTFM_KEY) {
        Write-Warning "No keys.env: this build will ask its users for Discord and Last.fm keys."
    }
    $env:TONEARM_VERSION = $tag
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}
if (-not (Test-Path target\release\tonearm.exe)) {
    throw "No target\release\tonearm.exe. Run without -SkipBuild."
}

$nsis = (Get-Command makensis -ErrorAction SilentlyContinue).Source
if (-not $nsis) {
    $nsis = @("${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe") |
        Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $nsis) { throw "NSIS is not installed. Run: winget install NSIS.NSIS" }
& $nsis -V3 "-DVERSION=$Version" installer\tonearm.nsi
if ($LASTEXITCODE -ne 0) { throw "makensis failed" }
Write-Host "Made tonearm-setup.exe for $tag"

if ($Publish) {
    # The bare exe is for 0.5 and older, whose updater looks for it.
    Copy-Item target\release\tonearm.exe tonearm.exe -Force
    gh release create $tag tonearm-setup.exe tonearm.exe --target (git rev-parse HEAD) --title "Tonearm $tag" --generate-notes
    if ($LASTEXITCODE -ne 0) { throw "gh release create failed" }
}
