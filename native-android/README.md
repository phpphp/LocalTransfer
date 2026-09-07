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

## 签名打包

```bash
# 1) 构建
gradle :app:assembleRelease
# 2) 对齐
$ANDROID_HOME/build-tools/36.0.0/zipalign -f 4 \
  app/build/outputs/apk/release/app-release-unsigned.apk aligned.apk
# 3) 签名（当前用 debug keystore；正式发布请生成自有 keystore 替换）
$ANDROID_HOME/build-tools/36.0.0/apksigner sign \
  --ks ~/.android/debug.keystore --ks-pass pass:android \
  --ks-key-alias androiddebugkey --key-pass pass:android \
  --out app-release-signed.apk aligned.apk
```

正式发布签名（自建 keystore，妥善保管，丢失则无法覆盖升级）：

```bash
keytool -genkey -v -keystore localtransfer.jks -alias localtransfer \
  -keyalg RSA -keysize 2048 -validity 10000
```
