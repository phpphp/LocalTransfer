# LocalTransfer

局域网文件与文本传输工具（类似飞鸽传书 / 飞秋 / LocalSend），Rust + GPUI 编写。

同一局域网内的设备互传文件、文件夹与文本，不走外网、无需账号。

## 功能

- **设备发现**：UDP 多播（`239.192.71.82:17878`）自动发现在线的 LocalTransfer 设备
- **文件/文件夹传输**：拖入或选择文件发送，对方确认后接收；实时进度、速度显示；可取消；文件夹保持目录结构
- **文本互传**：聊天消息 + 剪贴板一键粘贴发送/复制
- **消息历史**：本地 SQLite 保存，重启不丢
- **端口自动避让**：默认端口 17878 被占用时自动向后查找可用端口（通知栏提示），同机双开无需手动指定端口

## 构建与运行

支持 Windows / macOS / Linux 三桌面平台。

```powershell
# 需要 Rust (MSVC 工具链) + Visual Studio C++ 生成工具
cargo build --release
.\target\release\local-transfer.exe
```

macOS / Linux 的构建命令、系统依赖、安装包制作见 **[docs/BUILD.md](docs/BUILD.md)**。
推到 GitHub 后 Actions 会自动产出三平台安装包（打 tag 即发布 Release）。
移动端（Android/iOS）暂不支持——原因与替代方案见 BUILD.md 末尾。

首次运行 Windows 会弹出防火墙授权，请选择**允许访问**（勾选"专用网络"）。
也可以管理员身份运行脚本一键添加规则：

```powershell
.\scripts\add-firewall-rule.ps1
```

## 同机双实例调试

```powershell
.\target\release\local-transfer.exe --name PC-A
.\target\release\local-transfer.exe --name PC-B
```

带 `--name` 时使用临时身份（随机设备 ID，不写回配置）；HTTP 端口自动避让（第二个实例自动用下一个可用端口），无需手动指定。配置与历史保存在 `%APPDATA%\LocalTransfer\`。

## 使用说明

- 左侧选择目标设备 → 输入框回车发送文本；「文件」按钮选择文件/文件夹发送
- 「粘贴」按钮把剪贴板文本直接发给对方；对方消息卡片可复制（M4）
- 收到传输请求会弹窗确认，接收文件默认保存在 `下载\LocalTransfer`（可在设置中修改）
- 传输列表显示进度与速度，可随时取消

## 架构

```
crates/
├── core/    # transfer-core：纯 tokio 网络核心（发现/HTTP 服务/客户端/存储），不依赖 UI
└── app/     # GPUI 前端
```

### 协议（LocalTransfer Protocol v1）

| 端点 | 方法 | 说明 |
|---|---|---|
| UDP `239.192.71.82:53817` | - | `announce`/`bye` JSON 广播，25s 心跳，75s 超时离线 |
| `/api/info` | GET | 设备信息 |
| `/api/message` | POST | 文本消息 |
| `/api/transfer/prepare` | POST | 传输请求（接收方弹窗确认，60s 超时） |
| `/api/transfer/upload/{token}/{fileId}` | PUT | raw binary 流式上传 |
| `/api/transfer/cancel/{token}` | POST | 取消 |

文件夹传输为扁平文件列表（`relPath` 相对路径），接收端净化路径后重建目录。

## 开发

```powershell
cargo test -p transfer-core   # 协议/存储/收集 单元测试
cargo run -p local-transfer   # 调试运行
```
