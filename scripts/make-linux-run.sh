#!/bin/bash
# 生成 Linux 自解压安装包（.run）：LocalTransfer-Setup-<版本>-linux-x64.run
# 依赖：先跑 build-linux-wsl.ps1 产出 dist/<版本目录>/local-transfer
# 用法（git-bash）：bash scripts/make-linux-run.sh
set -e
cd "$(dirname "$0")/.."

VERSION=$(grep -m1 '^version = "' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
DIR="LocalTransfer-$VERSION-x86_64-unknown-linux-gnu"
SRC="dist/$DIR"
[ -f "$SRC/local-transfer" ] || { echo "缺 $SRC/local-transfer —— 先跑 scripts/build-linux-wsl.ps1"; exit 1; }

OUT="dist/LocalTransfer-Setup-$VERSION-linux-x64.run"
TMP=$(mktemp /tmp/lt-payload.XXXXXX.tar.gz)

# 载荷（根目录=版本文件夹）
tar czf "$TMP" -C dist "$DIR"

# 脚本头 + 标记 + base64 载荷（单行）
{
cat <<HEAD
#!/bin/bash
# LocalTransfer v$VERSION Linux 安装器（自解压）
# 用法：chmod +x LocalTransfer-Setup-$VERSION-linux-x64.run && ./LocalTransfer-Setup-$VERSION-linux-x64.run
# 安装位置：/usr/local/bin/local-transfer（需要 sudo 密码）
set -e
VERSION="$VERSION"

echo "== LocalTransfer v\$VERSION 安装 =="
if [ "\$(id -u)" != "0" ]; then
  echo "需要 root 权限（安装到 /usr/local），将请求 sudo…"
  exec sudo bash "\$0" "\$@"
fi

TMPD="\$(mktemp -d /tmp/localtransfer.XXXXXX)"
# 自解压：标记行之后是 base64 载荷
MARK_LINE=\$(grep -n '^__PAYLOAD__\$' "\$0" | head -1 | cut -d: -f1)
tail -n +\$((MARK_LINE + 1)) "\$0" | base64 -d | tar xz -C "\$TMPD"

SRC="\$TMPD/$DIR"

# 1) 程序 → /usr/local/bin
install -Dm755 "\$SRC/local-transfer" /usr/local/bin/local-transfer

# 2) 图标：绝对路径版 + hicolor 主题目录版（Wayland app_id 查图标走主题路径，
#    GNOME 任务栏对 Icon= 绝对路径的支持不一致，两处都放才稳妥）
install -Dm644 "\$SRC/icon.png" /usr/local/share/icons/localtransfer.png
install -Dm644 "\$SRC/icon.png" /usr/local/share/icons/hicolor/512x512/apps/localtransfer.png

# 3) 桌面菜单项（Exec/Icon 指向安装位置，不依赖解压目录）
mkdir -p /usr/local/share/applications
cat > /usr/local/share/applications/localtransfer.desktop <<'DESK'
[Desktop Entry]
Type=Application
Name=LocalTransfer
Comment=局域网文件/文本传输
Exec=/usr/local/bin/local-transfer
Icon=/usr/local/share/icons/localtransfer.png
Terminal=false
Categories=Network;FileTransfer;
DESK
update-desktop-database /usr/local/share/applications 2>/dev/null || true

rm -rf "\$TMPD"
echo "== 安装完成 =="
echo "  程序：/usr/local/bin/local-transfer（终端直接运行 local-transfer）"
echo "  菜单：应用列表 → 网络 → LocalTransfer"
echo ""
echo "首次运行请放行防火墙（如启用）："
echo "  ufw allow 17878/udp && ufw allow 17878/tcp"
exit 0

__PAYLOAD__
HEAD
base64 -w0 "$TMP"
echo
} > "$OUT"

rm -f "$TMP"
SIZE=$(du -m "$OUT" | cut -f1)
echo "$OUT ${SIZE}MB"
