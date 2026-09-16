package io.github.localtransfer

import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.net.wifi.WifiManager
import android.app.DownloadManager
import android.os.Build
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.MediaStore
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import java.io.File

class MainActivity : FlutterActivity() {
    private val multicastChannel = "localtransfer/multicast"
    private val downloadChannel = "localtransfer/downloads"
    private var multicastLock: WifiManager.MulticastLock? = null

    // "存到…"目录选择（ACTION_OPEN_DOCUMENT_TREE）的挂起结果
    private var pendingPickResult: MethodChannel.Result? = null

    companion object {
        private const val REQ_PICK_SAVE_DIR = 47001
    }

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
                        val treeUriStr = call.argument<String?>("treeUri")
                        val segs = rel.split('/')
                            .filter { it.isNotEmpty() && it != "." && it != ".." && !it.contains(':') }
                        val displayName = segs.lastOrNull() ?: File(src).name
                        val subDir = if (segs.size > 1) segs.dropLast(1).joinToString("/") else ""
                        try {
                            // "存到…"选定的 SAF 目录：DocumentsContract 建文件写入，
                            // 没有真实路径可返回，只回 content URI（打开/分享够用）
                            val realPath: String? = if (treeUriStr != null) {
                                val treeRoot = Uri.parse(treeUriStr)
                                var dir = DocumentsContract.buildDocumentUriUsingTree(
                                    treeRoot, DocumentsContract.getTreeDocumentId(treeRoot)
                                )
                                if (segs.size > 1) {
                                    for (s in segs.subList(0, segs.size - 1)) {
                                        dir = findOrCreateChild(treeRoot, dir, s)
                                    }
                                }
                                val mime = mimeForFile(displayName)
                                val fileUri = findOrCreateChild(treeRoot, dir, displayName, mime)
                                contentResolver.openOutputStream(fileUri)?.use { out ->
                                    File(src).inputStream().use { it.copyTo(out) }
                                } ?: throw IllegalStateException("打开输出流失败")
                                "null|$fileUri"
                            } else if (Build.VERSION.SDK_INT >= 29) {
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
                                // 返回 "path|uri"：真实路径（可能为 null）+ MediaStore URI（打开用）
                                val realPath = contentResolver.query(
                                    uri, arrayOf(MediaStore.MediaColumns.DATA),
                                    null, null, null
                                )?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
                                "$realPath|${uri.toString()}"
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
                    // "存到…"：系统目录选择器（SAF），返回可持续授权的树 URI
                    "pickSaveDir" -> {
                        if (pendingPickResult != null) {
                            result.error("BUSY", null, null); return@setMethodCallHandler
                        }
                        pendingPickResult = result
                        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
                            addFlags(
                                Intent.FLAG_GRANT_READ_URI_PERMISSION or
                                    Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
                                    Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
                            )
                        }
                        try {
                            @Suppress("DEPRECATION")
                            startActivityForResult(intent, REQ_PICK_SAVE_DIR)
                        } catch (e: Exception) {
                            pendingPickResult = null
                            result.error("PICK_FAILED", e.message, null)
                        }
                    }
                    "openUri" -> {
                        val uriStr = call.argument<String>("uri") ?: run {
                            result.error("NO_URI", null, null); return@setMethodCallHandler
                        }
                        val mime = call.argument<String>("mime") ?: "*/*"
                        try {
                            val intent = Intent(Intent.ACTION_VIEW).apply {
                                setDataAndType(Uri.parse(uriStr), mime)
                                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                            }
                            startActivity(intent)
                            result.success(true)
                        } catch (e: Exception) {
                            result.error("OPEN_FAILED", e.message, null)
                        }
                    }
                    // 在系统文件管理器（"文件"应用）中打开并定位到 Download 下的目录；
                    // 定位失败回退：打开系统下载列表
                    "openFolder" -> {
                        val rel = call.argument<String>("rel") ?: "LocalTransfer"
                        val safe = rel.split('/')
                            .filter { it.isNotEmpty() && it != "." && it != ".." }
                            .joinToString("/")
                        try {
                            val docUri = DocumentsContract.buildDocumentUri(
                                "com.android.externalstorage.documents",
                                "primary:Download/$safe"
                            )
                            val intent = Intent(Intent.ACTION_VIEW).apply {
                                setDataAndType(docUri, "vnd.android.document/directory")
                            }
                            startActivity(intent)
                            result.success(true)
                        } catch (e: Exception) {
                            try {
                                startActivity(
                                    Intent(DownloadManager.ACTION_VIEW_DOWNLOADS)
                                )
                                result.success(true)
                            } catch (e2: Exception) {
                                result.error("OPEN_FAILED", "${e.message}; ${e2.message}", null)
                            }
                        }
                    }
                    else -> result.notImplemented()
                }
            }
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode == REQ_PICK_SAVE_DIR) {
            val res = pendingPickResult
                ?: return super.onActivityResult(requestCode, resultCode, data)
            pendingPickResult = null
            val uri = data?.data
            if (resultCode == RESULT_OK && uri != null) {
                // 持久化授权：重启后仍可写（本批转存在 minutes 内发生，稳妥起见仍保留）
                try {
                    contentResolver.takePersistableUriPermission(
                        uri,
                        Intent.FLAG_GRANT_READ_URI_PERMISSION or
                            Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                    )
                } catch (_: SecurityException) {
                    // 部分提供方不支持持久化授权，本批写入不受影响
                }
                res.success(uri.toString())
            } else {
                res.success(null)
            }
            return
        }
        super.onActivityResult(requestCode, resultCode, data)
    }

    /// 在 SAF 目录下找同名子项（文件/目录），没有则按 [mime] 新建
    private fun findOrCreateChild(
        treeRoot: Uri,
        dirDoc: Uri,
        name: String,
        mime: String = "vnd.android.document/directory"
    ): Uri {
        val childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(
            treeRoot, DocumentsContract.getDocumentId(dirDoc)
        )
        contentResolver.query(
            childrenUri,
            arrayOf(
                DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                DocumentsContract.Document.COLUMN_DISPLAY_NAME
            ),
            null, null, null
        )?.use { c ->
            while (c.moveToNext()) {
                if (c.getString(1) == name) {
                    return DocumentsContract.buildDocumentUriUsingTree(treeRoot, c.getString(0))
                }
            }
        }
        return DocumentsContract.createDocument(contentResolver, dirDoc, mime, name)
            ?: throw IllegalStateException("createDocument 失败: $name")
    }

    /// 按扩展名给 DocumentsContract 建文件用的 MIME（SAF 场景尽量准确，
    /// 部分文档提供方按 MIME 决定文件归类）
    private fun mimeForFile(name: String): String {
        val ext = name.substringAfterLast('.', "").lowercase()
        return when (ext) {
            "jpg", "jpeg" -> "image/jpeg"
            "png" -> "image/png"
            "gif" -> "image/gif"
            "webp" -> "image/webp"
            "bmp" -> "image/bmp"
            "heic", "heif" -> "image/heic"
            "mp4" -> "video/mp4"
            "webm" -> "video/webm"
            "mkv" -> "video/x-matroska"
            "mov" -> "video/quicktime"
            "mp3" -> "audio/mpeg"
            "wav" -> "audio/wav"
            "pdf" -> "application/pdf"
            "txt", "md", "log" -> "text/plain"
            "apk" -> "application/vnd.android.package-archive"
            else -> "application/octet-stream"
        }
    }

    override fun onDestroy() {
        multicastLock?.release()
        multicastLock = null
        super.onDestroy()
    }
}
