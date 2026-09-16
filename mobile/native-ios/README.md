# LocalTransfer iOS 原生客户端（SwiftUI + Network.framework）

与 Flutter/Android 版功能对齐的原生实现。协议一致：UDP 多播 + HTTP JSON + 流式上传。
**本仓库无 macOS 构建环境，代码未经编译验证**——在 Xcode 中首次构建可能有小修。

## 功能

- 发现三层兜底：UDP 多播（Network.framework 多播组）→ 网段扫描（启动 + 45s）→ TCP 保活（5s 合并心跳）
- 收发文本；发文件（系统 fileImporter 多选，URLSession 流式上传带进度）
- 接收：确认 Alert → 消息流进度卡 → 完成合并文件夹卡（应用内文件清单逐个打开）
- 接收文件存 `文稿/Inbox`（Files App 可见）
- 手动添加（持久化 UserDefaults）、15s 离线判定、退出广播 bye

## Xcode 建立工程（一次性）

1. Xcode → Create New Project → iOS App
   - Product Name: `LocalTransfer`
   - Interface: **SwiftUI**，Language: **Swift**
   - 保存到本目录旁（或任意位置）
2. 删除模板生成的 `ContentView.swift` 和 `LocalTransferApp.swift`
3. 把 `LocalTransfer/` 下五个 Swift 文件拖进工程（勾选 Copy if needed + target）
4. Target → Info 添加（本地网络权限，iOS 14+ 必需）：
   - `NSLocalNetworkUsageDescription` = 在局域网内发现设备并传输文件
   - `NSBonjourServices` =（数组，可留空；纯多播不强制需要 Bonjour 类型）
5. Target → Signing & Capabilities → 勾选（App Sandbox 不适用于 iOS，无需）
6. Run（真机需开发者账号；模拟器多播受限，建议用 TCP 扫描/手动添加验证）

## 代码结构

| 文件 | 职责 |
|---|---|
| `Proto.swift` | 协议模型（DeviceInfo/FileMeta/JSON），与 proto.rs 对齐 |
| `Discovery.swift` | NWConnection 多播收发 + 互回节流 + 网段扫描 + 保活 |
| `MiniHTTPServer.swift` | NWListener TCP + 手写 HTTP/1.1（info/message/prepare/upload/cancel） |
| `TransferApi.swift` | URLSession：文本 / prepare→uploadTask 文件流式上传（delegate 进度） |
| `App.swift` | SwiftUI：设备列表 + 会话（气泡/文件卡/进度卡）+ 确认 Alert |

## 与 Android/Flutter 版的差异

- 接收文件存 App 沙盒 `Documents/Inbox`（iOS 无公共下载目录；Files App 可见）
- 打开文件直接 `UIApplication.open(url)`（沙盒内 URL）；无 MediaStore 概念
- 扫码未实现（iOS 相机扫码需 AVCaptureSession，可后续加；手动添加可用）
