# 在 WSL Ubuntu-24.04 里本机构建 Linux x64 包（产物放 dist\）。
# 首次运行会装系统依赖与 Rust（清华镜像）；源码每次重新拷入 WSL（快）。
# 用法: powershell -ExecutionPolicy Bypass -File scripts\build-linux-wsl.ps1
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot\..

$wsl = "wsl"

Write-Output "== 1/4 系统依赖（幂等） =="
& wsl -d Ubuntu-24.04 -u root -- bash -c "apt-get install -y -qq build-essential pkg-config libxkbcommon-dev libxkbcommon-x11-dev libx11-dev libxi-dev libxcursor-dev libxrandr-dev libxinerama-dev libwayland-dev libgl1 libegl1 libasound2-dev libfontconfig1-dev libfreetype-dev 2>&1 | tail -1"

Write-Output "== 2/4 Rust 工具链（已装则跳过） =="
& wsl -d Ubuntu-24.04 -u root -- bash -c "test -x /root/.cargo/bin/cargo || (curl -sSf https://sh.rustup.rs | RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup sh -s -- -y --default-toolchain stable --profile minimal) 2>&1 | tail -1"

Write-Output "== 3/4 拷源码 + 构建 =="
& wsl -d Ubuntu-24.04 -u root -- bash -c "rm -rf /root/localtransfer && mkdir -p /root/localtransfer && cp -r /mnt/c/Users/song/AI/claudeCode/LocalTransfer/crates /mnt/c/Users/song/AI/claudeCode/LocalTransfer/Cargo.toml /mnt/c/Users/song/AI/claudeCode/LocalTransfer/Cargo.lock /mnt/c/Users/song/AI/claudeCode/LocalTransfer/assets /root/localtransfer/"
& wsl -d Ubuntu-24.04 -u root -- bash -lc "source ~/.cargo/env; cd /root/localtransfer && cargo build --release 2>&1 | tail -3"
if ($LASTEXITCODE -ne 0) { Write-Output "BUILD FAILED"; exit 1 }

Write-Output "== 4/4 打包到 dist\ =="
$dst = "dist\LocalTransfer-0.1.0-x86_64-unknown-linux-gnu"
New-Item -ItemType Directory -Force $dst | Out-Null
& wsl -d Ubuntu-24.04 -u root -- bash -c "strip /root/localtransfer/target/release/local-transfer && cp /root/localtransfer/target/release/local-transfer /mnt/c/Users/song/AI/claudeCode/LocalTransfer/dist/LocalTransfer-0.1.0-x86_64-unknown-linux-gnu/"
if ($LASTEXITCODE -ne 0) { Write-Output "COPY FAILED"; exit 1 }
Copy-Item assets/README.txt $dst -Force
Copy-Item assets/icon.png $dst -Force
Copy-Item assets/localtransfer.desktop $dst -Force
tar czf "$dst.tar.gz" $dst
Get-Item "$dst.tar.gz" | Select-Object Name, @{n='MB';e={[math]::Round($_.Length/1MB,1)}}
Write-Output "== done =="
