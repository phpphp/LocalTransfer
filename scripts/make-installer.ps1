# 打包 Windows 分发包：
#   1) 绿色包 LocalTransfer-x.x.x-win64.zip（exe + install.ps1 + add-firewall-rule.ps1 + README.txt）
#   2) Inno Setup 安装包 LocalTransfer-Setup-x.x.x-win64.exe（需已装 Inno Setup 6）
# 用法: powershell -ExecutionPolicy Bypass -File scripts\make-installer.ps1
$ErrorAction = "Stop"
Set-Location $PSScriptRoot\..

$Version = "0.2.0"
$Iscc = "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"

Write-Output "== 构建 release =="
Get-Process -Name local-transfer -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 600
# 机器时钟跳变会让 cargo 增量误判 up-to-date、产出不含新代码的旧 exe（发生过多次）；
# 每次清掉 app crate 的 release 产物强制重编（依赖缓存保留，多花 ~40s 换确定性）
cargo clean --release -p local-transfer
if ($LASTEXITCODE -ne 0) { Write-Output "CLEAN FAILED"; exit 1 }
cargo build --release
if ($LASTEXITCODE -ne 0) { Write-Output "BUILD FAILED"; exit 1 }

# ---- 绿色包 ----
Write-Output "== 绿色包 =="
$stage = "dist\LocalTransfer-$Version-win64"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\local-transfer.exe $stage
Copy-Item scripts\install.ps1 $stage
Copy-Item scripts\add-firewall-rule.ps1 $stage
Copy-Item assets\README.txt $stage
Copy-Item assets\icon.ico $stage
Copy-Item assets\icon-normal.png $stage
Copy-Item assets\icon-badge.png $stage
$zip = "dist\LocalTransfer-$Version-win64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path "$stage\*" -DestinationPath $zip
Remove-Item $stage -Recurse -Force
Get-Item $zip | Select-Object Name, @{n='MB';e={[math]::Round($_.Length/1MB,1)}}

# ---- Inno Setup 安装包 ----
Write-Output "== Inno Setup 安装包 =="
if (-not (Test-Path $Iscc)) {
    Write-Output "未找到 ISCC.exe（先装 Inno Setup 6：winget install JRSoftware.InnoSetup）"
    exit 1
}
$setupExe = "dist\LocalTransfer-Setup-$Version-win64.exe"
if (Test-Path $setupExe) { Remove-Item $setupExe -Force }
& $Iscc "installer\localtransfer.iss" | Select-Object -Last 3
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $setupExe)) { Write-Output "ISCC FAILED"; exit 1 }
Get-Item $setupExe | Select-Object Name, @{n='MB';e={[math]::Round($_.Length/1MB,1)}}
Write-Output "== 完成 =="
