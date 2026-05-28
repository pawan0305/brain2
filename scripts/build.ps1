#!/usr/bin/env pwsh
# One-shot build: install JS deps (if needed) and produce the release binary
# plus the NSIS installer under src-tauri\target\release\bundle\nsis\.
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not (Test-Path "node_modules")) {
    Write-Host "==> npm install"
    npm install
}

Write-Host "==> npm run tauri build"
npm run tauri build

$installer = Get-ChildItem "src-tauri\target\release\bundle\nsis\*-setup.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($installer) {
    Write-Host "`n==> Installer: $($installer.FullName)"
} else {
    Write-Warning "Build finished but no NSIS installer was found."
}
