# Rebuild, relaunch two instances, and capture DPI-aware screenshots.
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\dev-check.ps1
$ErrorActionPreference = "Continue"
Set-Location $PSScriptRoot\..

Write-Output "== stopping running instances =="
Get-Process -Name local-transfer -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 800

Write-Output "== cargo build =="
cargo build
if ($LASTEXITCODE -ne 0) { Write-Output "BUILD FAILED"; exit 1 }

Write-Output "== launching two instances =="
Start-Process -FilePath ".\target\debug\local-transfer.exe" -ArgumentList '--name','Alpha'
Start-Sleep -Seconds 3
Start-Process -FilePath ".\target\debug\local-transfer.exe" -ArgumentList '--name','Beta'
Start-Sleep -Seconds 5

Write-Output "== screenshots =="
powershell -ExecutionPolicy Bypass -File scripts\screenshot.ps1 -Out shot_new_a.png -Index 0
powershell -ExecutionPolicy Bypass -File scripts\screenshot.ps1 -Out shot_new_b.png -Index 1
Write-Output "== done =="
