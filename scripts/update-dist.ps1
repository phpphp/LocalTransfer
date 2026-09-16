# Full rebuild + dist refresh, run this after every change set:
#   1) Desktop (Rust): target\debug + target\release
#   2) Windows packages into dist\ (portable zip + Inno setup exe)
#   3) Android native: debug + release APKs, copied next to the project and into dist\
# Usage: & .\scripts\update-dist.ps1   (or powershell -ExecutionPolicy Bypass -File scripts\update-dist.ps1)
# NOTE: do NOT set $ErrorActionPreference="Stop" here — under PowerShell 5.1 a
# native command (cargo/gradle) writing to stderr while the caller pipes with
# 2>&1 turns each stderr line into an ErrorRecord and would abort the script.
# Failures are handled by explicit $LASTEXITCODE checks below.
$ErrorActionPreference = "Continue"
Set-Location $PSScriptRoot\..

# Version comes from the workspace Cargo.toml (keep make-installer.ps1 / localtransfer.iss in sync)
$Version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' |
    Select-Object -First 1).Matches.Groups[1].Value
Write-Output "== version $Version =="

# ---- Desktop (Rust): debug + release ----
Write-Output "== cargo build (debug + release) =="
Get-Process -Name local-transfer -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 600
cargo build -p local-transfer
if ($LASTEXITCODE -ne 0) { Write-Output "DEBUG BUILD FAILED"; exit 1 }
cargo build --release -p local-transfer
if ($LASTEXITCODE -ne 0) { Write-Output "RELEASE BUILD FAILED"; exit 1 }

# ---- Windows dist packages (portable zip + Inno setup; reuses the release build) ----
& .\scripts\make-installer.ps1
if ($LASTEXITCODE -ne 0) { Write-Output "INSTALLER PACK FAILED"; exit 1 }

# ---- Android native: debug + release ----
Write-Output "== android build =="
$env:JAVA_HOME = 'C:\Program Files\Android\openjdk\jdk-21.0.8'
$env:ANDROID_HOME = 'C:\android-sdk'
# Gradle：优先 scoop 全局安装；回退 wrapper dists 里版本最高的。
# （-First 1 曾在 dists 混入多个版本时挑中最旧的 8.13，AGP 9 直接拒构建）
$Gradle = @("$env:USERPROFILE\scoop\apps\gradle\current\bin\gradle.bat") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $Gradle) {
    $Gradle = Get-ChildItem "$env:USERPROFILE\.gradle\wrapper\dists\gradle-*\*\gradle-*\bin\gradle.bat" |
        Sort-Object { [version](($_.FullName -replace '.*dists[\\/]gradle-', '') -replace '[\\\/].*$', '') } -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $Gradle) { Write-Output "GRADLE NOT FOUND in wrapper dists"; exit 1 }
Push-Location mobile/native-android
& $Gradle assembleDebug assembleRelease --console=plain | Select-Object -Last 3
$androidOk = $LASTEXITCODE
Pop-Location
if ($androidOk -ne 0) { Write-Output "ANDROID BUILD FAILED"; exit 1 }

Copy-Item mobile/native-android\app\build\outputs\apk\release\app-release.apk mobile/native-android\app-release.apk -Force
Copy-Item mobile/native-android\app\build\outputs\apk\debug\app-debug.apk mobile/native-android\app-debug.apk -Force
Copy-Item mobile/native-android\app\build\outputs\apk\release\app-release.apk "dist\LocalTransfer-$Version-android-native.apk" -Force

Write-Output "== done =="
Write-Output "-- dist --"
Get-ChildItem dist\*$Version* | Select-Object Name, @{n = 'MB'; e = { [math]::Round($_.Length / 1MB, 1) } }
Write-Output "-- android apks --"
Get-Item mobile/native-android\app-debug.apk, mobile/native-android\app-release.apk |
    Select-Object Name, @{n = 'MB'; e = { [math]::Round($_.Length / 1MB, 1) } }
Write-Output "-- rust exes --"
Get-Item target\debug\local-transfer.exe, target\release\local-transfer.exe |
    Select-Object Name, LastWriteTime
