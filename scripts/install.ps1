# LocalTransfer 安装脚本（随安装包分发）。
# 功能：停止运行中的旧版本 → 版本检查 → 覆盖安装 → 桌面/开始菜单快捷方式
#       → 控制面板卸载项。全部 per-user，无需管理员权限。
# 用法：powershell -ExecutionPolicy Bypass -File install.ps1 [-Uninstall]
param([switch]$Uninstall)

$ErrorAction = "Stop"
$AppName = "LocalTransfer"
$Version = "0.1.0"
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
$Exe = Join-Path $InstallDir "local-transfer.exe"
$UninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$AppName"

function Remove-Shortcuts {
    foreach ($dir in @([Environment]::GetFolderPath("Desktop"); [Environment]::GetFolderPath("Programs"))) {
        $lnk = Join-Path $dir "$AppName.lnk"
        if (Test-Path $lnk) { Remove-Item $lnk -Force }
    }
}

if ($Uninstall) {
    Write-Output "== 卸载 $AppName =="
    Get-Process -Name local-transfer -ErrorAction SilentlyContinue | Stop-Process -Force
    Remove-Shortcuts
    if (Test-Path $UninstallKey) { Remove-Item $UninstallKey -Force }
    if (Test-Path $InstallDir) { Remove-Item $InstallDir -Recurse -Force }
    Write-Output "已卸载（配置与聊天记录保留在 %APPDATA%\LocalTransfer\）"
    exit 0
}

Write-Output "== 安装 $AppName v$Version =="
if ([Environment]::Is64BitOperatingSystem -eq $false) {
    Write-Output "仅支持 64 位 Windows"; exit 1
}

# 1) 老版本检查
$oldVersion = $null
if (Test-Path $Exe) {
    $oldVersion = (Get-Item $Exe).VersionInfo.FileVersion
    if ($oldVersion -eq $Version) {
        Write-Output "已安装相同版本 v$Version，执行覆盖安装"
    } else {
        Write-Output "检测到旧版本 v$oldVersion，将升级到 v$Version"
    }
    # 停止正在运行的旧版本（否则文件被占用换不出新 exe）
    Get-Process -Name local-transfer -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 600
}

# 2) 覆盖安装
New-Item -ItemType Directory -Force $InstallDir | Out-Null
Copy-Item "$PSScriptRoot\local-transfer.exe" $InstallDir -Force
Copy-Item "$PSScriptRoot\README.txt" $InstallDir -Force -ErrorAction SilentlyContinue

# 3) 快捷方式（桌面 + 开始菜单；图标用 exe 内嵌图标）
$ws = New-Object -ComObject WScript.Shell
foreach ($dir in @([Environment]::GetFolderPath("Desktop"); [Environment]::GetFolderPath("Programs"))) {
    $lnk = $ws.CreateShortcut((Join-Path $dir "$AppName.lnk"))
    $lnk.TargetPath = $Exe
    $lnk.WorkingDirectory = $InstallDir
    $lnk.Description = "局域网文件与文本传输"
    $lnk.Save()
}

# 4) 控制面板"应用与功能"卸载项
New-Item -Path $UninstallKey -Force | Out-Null
Set-ItemProperty $UninstallKey -Name DisplayName -Value $AppName
Set-ItemProperty $UninstallKey -Name DisplayVersion -Value $Version
Set-ItemProperty $UninstallKey -Name DisplayIcon -Value $Exe
Set-ItemProperty $UninstallKey -Name Publisher -Value "LocalTransfer"
Set-ItemProperty $UninstallKey -Name UninstallString -Value "powershell -ExecutionPolicy Bypass -File `"$InstallDir\uninstall.ps1`""

# 5) 卸载脚本落在安装目录
@"
# LocalTransfer 卸载脚本（由安装时生成）
powershell -ExecutionPolicy Bypass -File `"$InstallDir\install.ps1`" -Uninstall
"@ | Out-File (Join-Path $InstallDir "uninstall.ps1") -Encoding utf8

Write-Output "安装完成：$Exe"
if ($oldVersion -and $oldVersion -ne $Version) { Write-Output "（已从 v$oldVersion 升级）" }
Write-Output "桌面已创建快捷方式；可在""应用与功能""中卸载"
