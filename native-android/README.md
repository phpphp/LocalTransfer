# LocalTransfer Android 原生客户端（Kotlin + Jetpack Compose）

与 Flutter 版（`../mobile/`）功能对齐的原生实现。协议完全一致：
UDP 多播发现（239.192.71.82:17878）+ HTTP JSON + 流式上传。

## 功能

- 三层发现兜底：UDP 多播/广播 → TCP 网段扫描（启动 + 45s 周期）→ TCP 保活（6s）
- 收发文本；发文件（系统选择器多选，流式上传带进度）
- 接收：确认对话框 → 消息流进度卡 → 完成合并为文件夹卡片
- 接收文件经 MediaStore 落 `Download/LocalTransfer/`（保持目录结构），URI 打开
- 扫码添加（zxing-embedded）、15s 离线判定、退出广播 bye
- 手动添加持久化（SharedPreferences）

## 构建

需要 JDK 17+、Android SDK（platform 36）。gradle/maven 已配国内镜像。

```bash
cd native-android
gradle :app:assembleRelease   # 或用 Android Studio 打开直接 Run
# 产物：app/build/outputs/apk/release/app-release-unsigned.apk
```

gradle wrapper 未包含（复用全局 gradle 或 `gradle wrapper` 生成）。

## 代码结构

| 文件 | 职责 |
|---|---|
| `Proto.kt` | 协议模型（DeviceInfo/FileMeta/JSON），与 proto.rs 对齐 |
| `Discovery.kt` | 多播收发 + 单播互回（2s 节流）+ 网段扫描 + 保活 + MulticastLock |
| `MiniHttpServer.kt` | 手写 HTTP/1.1：info/message/prepare/upload/cancel + MediaStore 转存 |
| `TransferApi.kt` | HttpURLConnection 客户端：文本 / prepare→流式上传（先取流再写，防反压死锁） |
| `MainActivity.kt` | Compose UI：设备列表 + 会话（气泡/文件卡/进度卡）+ 确认对话框 + 扫码 |

## 与 Flutter 版的差异

- UI 用 Compose Material3（深色主题固定 indigo 主色）
- HTTP 服务是手写的迷你实现（Flutter 用 shelf），路由语义一致
- 文件选择用系统 `GetMultipleContents`（无需 file_picker 插件）

## 签名打包（已配置正式证书）

正式证书 `localtransfer.jks` + `keystore.properties`（均不入 git，密码与备份见本地 `SIGNING.md`）。
gradle 构建时自动签名：

```bash
gradle :app:assembleRelease
# 产物（已签名+对齐）：app/build/outputs/apk/release/app-release.apk
```

证书缺失时自动回落 debug 签名（新克隆的开发机直接可构建）。
