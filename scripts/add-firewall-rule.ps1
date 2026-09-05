# LocalTransfer 防火墙规则（需要管理员权限运行）
# 允许程序在专用网络入站（UDP 发现 + HTTP 传输）

#Requires -RunAsAdministrator

$exe = Join-Path $PSScriptRoot "..\target\release\local-transfer.exe"
if (-not (Test-Path $exe)) {
    $exe = Join-Path $PSScriptRoot "..\target\debug\local-transfer.exe"
}
if (-not (Test-Path $exe)) {
    # 允许手动传入路径
    if ($args.Count -gt 0 -and (Test-Path $args[0])) {
        $exe = (Resolve-Path $args[0]).Path
    } else {
        Write-Host "未找到 local-transfer.exe，请先构建或将其路径作为参数传入" -ForegroundColor Yellow
        exit 1
    }
}
$exe = (Resolve-Path $exe).Path

Write-Host "为 $exe 添加防火墙入站规则（专用网络）..."
netsh advfirewall firewall delete rule name="LocalTransfer" | Out-Null
netsh advfirewall firewall add rule `
    name="LocalTransfer" dir=in action=allow program="$exe" `
    enable=yes profile=private | Out-Null

if ($LASTEXITCODE -eq 0) {
    Write-Host "完成。首次运行程序弹出的防火墙提示仍可安全选择『允许』。" -ForegroundColor Green
} else {
    Write-Host "添加失败，请检查是否以管理员身份运行。" -ForegroundColor Red
    exit 1
}
