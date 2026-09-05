# LocalTransfer 移动客户端（Flutter）

与桌面端（Rust+GPUI，`crates/`）互通的 Android / iOS 客户端。协议完全复用桌面端定义：
UDP 多播发现（`239.192.71.82:17878`）+ HTTP JSON + 流式上传（见 `../crates/core/src/proto.rs`）。

## 功能

- **设备发现**：加入多播组监听 announce/bye，周期广播自己；与桌面端互相发现
- **本机服务**（shelf，端口 17878 起自动避让）：桌面端可主动发消息/文件给手机
- **发文本**：POST /api/message（桌面端聊天会话实时出现）
- **发文件**：file_picker 多选 → prepare（桌面端确认）→ 逐文件流式上传（带进度）
- **接收**：桌面端发来时弹确认卡片，接受后落盘到应用外部目录（文件管理器可见）
- **退出下线**：广播 bye，桌面端立即感知

## 首次构建

本目录不含 `android/`、`ios/` 平台脚手架（由 Flutter 工具生成）。装好 Flutter SDK 后：

```bash
cd mobile
flutter create . --org io.github --project-name localtransfer \
  --platforms=android,ios   # 生成平台目录（不会覆盖 lib/ 与 pubspec.yaml）
flutter pub get
flutter run                 # 连接设备/模拟器
flutter build apk --release
# iOS: flutter build ipa（需要 macOS + Xcode）
```

### Android 必要配置（android/app/src/main/AndroidManifest.xml）

`<manifest>` 下加权限（`</application>` 之后、`</manifest>` 之前）：

```xml
<uses-permission android:name="android.permission.INTERNET" />
<uses-permission android:name="android.permission.CHANGE_WIFI_MULTICAST_STATE" />
```

接收的文件保存在应用外部专项目录（`Android/data/io.github.localtransfer/files/../LocalTransfer`），
无需存储权限；如需存入公共下载目录，后续版本用 MediaStore 实现。

iOS 无需额外权限配置（Info.plist 的本地网络描述可加：
`NSLocalNetworkUsageDescription` = 访问局域网传输文件）。

## 代码结构

| 文件 | 职责 |
|---|---|
| `lib/proto.dart` | 协议模型（DeviceInfo/FileMeta/发现包编解码），与 proto.rs 对应 |
| `lib/discovery.dart` | UDP 多播发现（peer 注册表 + 心跳 + 单播互发现 + bye） |
| `lib/api.dart` | 桌面端 API 客户端（文本、prepare→流式上传、取消） |
| `lib/server.dart` | 本机 shelf 服务（桌面端视角里手机是一台正常对端） |
| `lib/main.dart` | UI：设备列表页 + 会话页（Material 3，明暗自适应） |

## 已知边界（v0.1）

- 接收侧整读写入（shelf 便捷路径），单文件 >1GB 时内存占用高——后续用 hijack 流式
- 桌面端发文件夹到手机：文件逐个到达并显示为多张卡片（无批次分组 UI）
- 后台保活未做：应用切后台后接收可能延迟（前台一切正常）
