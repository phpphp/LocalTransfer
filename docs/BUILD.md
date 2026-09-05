# 各平台构建指南

LocalTransfer 支持三个桌面平台（同一份代码）：

| 平台 | 状态 | 产物 |
|---|---|---|
| Windows x64 | ✅ 常规 | zip（exe 绿色单文件） |
| macOS (Apple Silicon / Intel) | ✅ | .app / .dmg |
| Linux (x64 / arm64) | ✅ | tar.gz + .desktop |
| Android / iOS | ❌ 见下文 | — |

## 快捷方式：GitHub Actions（无需本机交叉编译）

仓库自带 `.github/workflows/release.yml`。推到 GitHub 后：

- **拿安装包**：Actions → build → 任一次运行 → Artifacts 下载（Windows 绿色包+Inno 安装包、macOS 双架构 dmg、Linux 双架构 tar.gz，共 6 个包）
- **发正式版**：推一个 tag（如 `git tag v0.1.0 && git push --tags`）→ 自动创建 Release 并挂上全部安装包

Windows job 会自动 `choco install innosetup` 并用仓库里的 `installer\localtransfer.iss` 编译出安装包 exe。

## Windows

```powershell
cargo build --release
# 或一键产出两个分发包（见下）
```

分发打包（`scripts\make-installer.ps1`，需先装 Inno Setup 6：`winget install JRSoftware.InnoSetup`）：

| 产物 | 内容 |
|---|---|
| `dist\LocalTransfer-<版本>-win64.zip` | 绿色包：exe + install.ps1 + add-firewall-rule.ps1 + README.txt |
| `dist\LocalTransfer-Setup-<版本>-win64.exe` | Inno Setup 安装包：中文向导、桌面/开始菜单快捷方式（可选任务）、旧版本检测覆盖升级、"应用与功能"卸载项；per-user 免管理员 |

Inno 脚本在 `installer\localtransfer.iss`（改版本号同步改 `#define MyAppVersion`）。

## macOS

```bash
cargo build --release
./scripts/make-macos-app.sh            # 生成 dist/LocalTransfer.app
# 或打成 dmg：
hdiutil create -volname LocalTransfer -srcfolder dist/LocalTransfer.app -ov -format UDZO LocalTransfer.dmg
```

双架构分别构建：`cargo build --release --target aarch64-apple-darwin`（M 系列）/ `--target x86_64-apple-darwin`（Intel）。

首次运行被 Gatekeeper 拦截时：右键 App → 打开；或 `xattr -cr dist/LocalTransfer.app`。
macOS 14+ 首次启动会弹"允许访问本地网络"——必须允许，否则发现不了设备。

## Linux

```bash
# 依赖（Debian/Ubuntu；名字随发行版略有差异）
sudo apt install build-essential pkg-config libxkbcommon-dev libxkbcommon-x11-dev \
     libx11-dev libxi-dev libxcursor-dev libxrandr-dev libxinerama-dev \
     libwayland-dev libgl1 libegl1 libasound2-dev \
     libfontconfig1-dev libfreetype-dev

cargo build --release
strip target/release/local-transfer    # 62MB → 46MB
# 桌面集成（可选）：
sudo install -Dm755 target/release/local-transfer /usr/local/bin/
sudo install -Dm644 assets/icon.png /usr/local/share/icons/hicolor/256x256/apps/localtransfer.png
sudo install -Dm644 assets/localtransfer.desktop /usr/local/share/applications/
```

> 本机构建的包基于 Ubuntu 24.04（glibc 2.39）。老发行版（Ubuntu 20.04 等 glibc < 2.39）
> 上可能报 `GLIBC_x.xx not found` —— 那类系统建议本地源码构建或用 CI 产出的包。

### 没有 Linux 机器？两种本机替代

- **Windows + WSL2**（本仓库已配好一键脚本）：

  ```powershell
  powershell -ExecutionPolicy Bypass -File scripts\build-linux-wsl.ps1
  # 产物：dist\LocalTransfer-0.1.0-x86_64-unknown-linux-gnu.tar.gz
  ```

  首次运行自动装系统依赖与 Rust（清华镜像），之后每次只做增量构建。

- **GitHub Actions**：见上文，无需任何本地环境。

防火墙（如启用 ufw/firewalld）：放行 UDP 17878（发现）与 TCP 17878（传输）。

## Android / iOS：当前不可行

UI 框架 gpui（Zed 的 GPU UI 框架）**没有移动端后端**——只有 macOS / Linux / Windows / Web，
不可能把现有代码编译成 Android/iOS 包。

如果要手机端参与传输，现实路径是**另写一个移动端客户端**（协议本身很简单：
UDP 17878 多播发现 + HTTP JSON + 流式上传，见 `crates/core/src/proto.rs`）：
- Kotlin/Compose 或 Flutter 调 HTTP 即可复用现有网络协议，与桌面端互通
- 这是独立项目，需要 Android Studio 开发环境，不适合在本仓库里交叉生成

需要的话可以后续单独立项做移动端。
