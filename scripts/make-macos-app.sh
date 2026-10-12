#!/usr/bin/env bash
# 生成 macOS 的 LocalTransfer.app（双击运行）。
# 用法: ./scripts/make-macos-app.sh [二进制路径，默认 target/release/local-transfer]
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="${1:-target/release/local-transfer}"
[ -f "$BIN" ] || { echo "找不到二进制: $BIN（先 cargo build --release）"; exit 1; }

APP="dist/LocalTransfer.app/Contents"
mkdir -p "$APP/MacOS" "$APP/Resources"
cp "$BIN" "$APP/MacOS/LocalTransfer"
chmod +x "$APP/MacOS/LocalTransfer"

# 从 256x256 png 生成 icns（需要 macOS 自带 iconutil）
if [ -f assets/icon.png ] && command -v iconutil >/dev/null; then
  TMPIcon=$(mktemp -d)/icon.iconset
  mkdir -p "$TMPIcon"
  for sz in 16 32 64 128 256; do
    sips -z $sz $sz assets/icon.png --out "$TMPIcon/icon_${sz}x${sz}.png" >/dev/null
    sips -z $((sz*2)) $((sz*2)) assets/icon.png --out "$TMPIcon/icon_${sz}x${sz}@2x.png" >/dev/null
  done
  iconutil -c icns "$TMPIcon" -o "$APP/Resources/icon.icns"
fi

cat > "$APP/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>LocalTransfer</string>
  <key>CFBundleIdentifier</key><string>io.github.localtransfer</string>
  <key>CFBundleName</key><string>LocalTransfer</string>
  <key>CFBundleDisplayName</key><string>LocalTransfer</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundleHighResolutionCapable</key><true/>
  <!-- 局域网发现需要本地网络权限（macOS 14+ 会弹窗询问） -->
  <key>NSLocalNetworkUsageDescription</key>
  <string>LocalTransfer 需要访问本地网络来发现设备并传输文件</string>
  <key>NSBonjourServices</key>
  <array/>
</dict>
</plist>
PLIST

# ad-hoc 签名（无开发者账号也可签）：
# 未签名 app 会被 Gatekeeper/App Translocation 搬到随机只读路径运行，
# macOS 的"本地网络"隐私权限按签名指纹记账——每次构建的新未签名二进制
# 都被视为"另一个 app"，旧授权作废且可能不再弹窗（ multicast 收发全被静默拦截，
# 表现为"谁也发现不了谁"）。ad-hoc 签名让 app 结构合法、权限弹窗可靠出现。
if command -v codesign >/dev/null; then
  codesign --force --sign - dist/LocalTransfer.app
fi

# dmg 装载后展示 /Applications 快捷方式（拖拽安装的标准布局）
if [ -d dist/LocalTransfer.app ]; then
  rm -rf dist/.dmg-stage
  mkdir -p dist/.dmg-stage
  cp -R dist/LocalTransfer.app dist/.dmg-stage/
  ln -s /Applications dist/.dmg-stage/Applications
  echo "已生成 dist/LocalTransfer.app（首次运行如被 Gatekeeper 拦截：右键→打开）"
  echo "dmg 布局就绪（.dmg-stage：app + /Applications 快捷方式）"
fi
