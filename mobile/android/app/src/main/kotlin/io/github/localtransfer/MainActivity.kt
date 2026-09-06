package io.github.localtransfer

import android.content.ContentValues
import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import java.io.File

class MainActivity : FlutterActivity() {
    private val multicastChannel = "localtransfer/multicast"
    private val downloadChannel = "localtransfer/downloads"
    private var multicastLock: WifiManager.MulticastLock? = null

    override fun configureFlutterEngine(engine: FlutterEngine) {
        super.configureFlutterEngine(engine)

        // Android 的 WiFi 驱动默认过滤多播/广播帧（省电）；
        // 发现协议靠多播收 announce，必须持 MulticastLock 才收得到。
        MethodChannel(engine.dartExecutor.binaryMessenger, multicastChannel)
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "acquire" -> {
                        try {
                            if (multicastLock == null) {
                                val wifi = applicationContext
                                    .getSystemService(Context.WIFI_SERVICE) as WifiManager
                                multicastLock = wifi.createMulticastLock("localtransfer").apply {
                                    setReferenceCounted(false)
                                    acquire()
                                }
                            }
                            result.success(true)
                        } catch (e: Exception) {
                            result.error("LOCK_FAILED", e.message, null)
                        }
                    }
                    "release" -> {
                        try {
                            multicastLock?.release()
                            multicastLock = null
                            result.success(true)
                        } catch (e: Exception) {
                            result.error("RELEASE_FAILED", e.message, null)
                        }
                    }
                    else -> result.notImplemented()
                }
            }

        // 把接收的文件复制进公共下载目录（Download/LocalTransfer/...），
        // 文件管理器/电脑 USB 直接可见。返回复制后的真实路径（用于点击打开）。
        MethodChannel(engine.dartExecutor.binaryMessenger, downloadChannel)
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "save" -> {
                        val src = call.argument<String>("path") ?: run {
                            result.error("NO_PATH", null, null); return@setMethodCallHandler
                        }
                        val rel = call.argument<String>("rel") ?: ""
                        val segs = rel.split('/')
                            .filter { it.isNotEmpty() && it != "." && it != ".." && !it.contains(':') }
                        val displayName = segs.lastOrNull() ?: File(src).name
                        val subDir = if (segs.size > 1) segs.dropLast(1).joinToString("/") else ""
                        try {
                            val realPath: String? = if (Build.VERSION.SDK_INT >= 29) {
                                val values = ContentValues().apply {
                                    put(MediaStore.Downloads.DISPLAY_NAME, displayName)
                                    put(MediaStore.MediaColumns.MIME_TYPE, "application/octet-stream")
                                    put(
                                        MediaStore.Downloads.RELATIVE_PATH,
                                        "Download/LocalTransfer" + (if (subDir.isNotEmpty()) "/$subDir" else "")
                                    )
                                }
                                val uri = contentResolver.insert(
                                    MediaStore.Downloads.EXTERNAL_CONTENT_URI, values
                                ) ?: throw IllegalStateException("insert 失败")
                                contentResolver.openOutputStream(uri)?.use { out ->
                                    File(src).inputStream().use { it.copyTo(out) }
                                }
                                // 反查真实路径（多数设备可用；失败则返回 null，Dart 侧回退原路径）
                                contentResolver.query(
                                    uri, arrayOf(MediaStore.MediaColumns.DATA),
                                    null, null, null
                                )?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
                            } else {
                                @Suppress("DEPRECATION")
                                val dir = File(
                                    Environment.getExternalStoragePublicDirectory(
                                        Environment.DIRECTORY_DOWNLOADS
                                    ), "LocalTransfer" + (if (subDir.isNotEmpty()) "/$subDir" else "")
                                )
                                dir.mkdirs()
                                val dst = File(dir, displayName)
                                File(src).copyTo(dst, overwrite = true)
                                dst.absolutePath
                            }
                            if (realPath != null && realPath != src) File(src).delete()
                            result.success(realPath)
                        } catch (e: Exception) {
                            result.error("SAVE_FAILED", e.message, null)
                        }
                    }
                    else -> result.notImplemented()
                }
            }
    }

    override fun onDestroy() {
        multicastLock?.release()
        multicastLock = null
        super.onDestroy()
    }
}
