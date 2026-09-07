package io.github.localtransfer

import android.content.ContentValues
import android.content.Context
import android.os.Build
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.provider.MediaStore
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.InputStream
import java.net.ServerSocket
import java.net.Socket
import java.util.UUID
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit

class IncomingReq(
    val peer: DeviceInfo,
    val files: List<FileMeta>,
    val decision: CompletableFuture<Boolean>,
)

class RecvSession(
    val token: String,
    val peerId: String,
    val files: List<FileMeta>,
    val saveDir: File,
) {
    val done = ConcurrentHashMap.newKeySet<ReceivedFile>()
    @Volatile var completed = 0
}

private class Resp(val code: Int, val body: ByteArray = ByteArray(0),
                   val json: Boolean = false) {
    companion object {
        fun ok(body: ByteArray = ByteArray(0)) = Resp(200, body)
        fun okJson(s: String) = Resp(200, s.toByteArray(), json = true)
        fun err(code: Int, msg: String) = Resp(code, msg.toByteArray())
    }
}

/**
 * 请求体读取器：同时支持 Content-Length 定长与 Transfer-Encoding: chunked。
 *
 * 桌面端 reqwest 的流式上传（Body::wrap(StreamBody)）用 chunked 编码
 * 【没有 Content-Length 头】——只按 content-length 读会得到 0 字节并立即
 * 响应关连接，客户端还在写 → 对端报"网络错误"（0B 文件的根因）。
 */
private class BodyReader(private val input: InputStream,
                         headers: Map<String, String>) {
    private val chunked =
        headers["transfer-encoding"]?.contains("chunked", ignoreCase = true) == true
    private var fixedRemaining: Long =
        headers["content-length"]?.toLongOrNull() ?: 0L
    // chunked 状态
    private var chunkRemaining = 0L
    private var done = false

    /** 读一段 body 到 buf，返回字节数（0 = 结束，-1 = 出错） */
    fun read(buf: ByteArray): Int {
        if (done) return 0
        if (!chunked) {
            if (fixedRemaining <= 0) { done = true; return 0 }
            val n = input.read(buf, 0, minOf(buf.size.toLong(), fixedRemaining).toInt())
            if (n < 0) { done = true; return 0 }
            fixedRemaining -= n
            if (fixedRemaining <= 0) done = true
            return n
        }
        // chunked：当前块读完 → 读下一块的 size 行
        if (chunkRemaining <= 0) {
            if (!nextChunk()) return 0
        }
        val n = input.read(buf, 0, minOf(buf.size.toLong(), chunkRemaining).toInt())
        if (n < 0) { done = true; return -1 }
        chunkRemaining -= n
        if (chunkRemaining <= 0) readCrLf()   // 块尾 CRLF
        return n
    }

    /** 读下一块头；终止块（size=0）后吞掉 trailer 直到空行 */
    private fun nextChunk(): Boolean {
        val sizeLine = readLine(input) ?: return false
        val hex = sizeLine.substringBefore(';').trim()
        val size = hex.toLongOrNull(16) ?: return false
        if (size == 0L) {
            // trailer 节（可为空）：连续读到空行
            while (true) {
                val t = readLine(input) ?: break
                if (t.isEmpty()) break
            }
            done = true
            return false
        }
        chunkRemaining = size
        return true
    }

    private fun readCrLf() {
        if (input.read() == '\r'.code) input.read()  // \n
    }

    /** 读完整个 body（用于 JSON 请求体） */
    fun readAll(): ByteArray? {
        val out = ByteArrayOutputStream()
        val buf = ByteArray(16 * 1024)
        while (true) {
            val n = read(buf)
            if (n <= 0) break
            out.write(buf, 0, n)
        }
        return if (chunked || fixedRemaining >= 0) out.toByteArray() else null
    }
}

/**
 * 极简 HTTP/1.1 服务（路由与桌面端 axum 对齐）。
 * 手写解析：请求行 + 头 + 请求体（定长 / chunked）；一问一答后关闭。
 * 回调统一切到主线程（Compose 状态只能在主线程写）。
 */
class MiniHttpServer(
    me: DeviceInfo,
    private val context: Context,
    private val callbacks: Callbacks,
) {
    interface Callbacks {
        fun onMessage(peerId: String, peerName: String, text: String)
        fun onIncoming(req: IncomingReq)
        fun onProgress(token: String, p: RecvProgress)
        fun onBatchDone(peerId: String, files: List<ReceivedFile>)
    }

    private var server: ServerSocket? = null
    private val sessions = ConcurrentHashMap<String, RecvSession>()
    private val main = Handler(Looper.getMainLooper())
    @Volatile private var running = true

    @Volatile private var identity: DeviceInfo = me

    val port: Int get() = server?.localPort ?: 0

    /** 端口确定后回填身份（/api/info 返回它；构造时 port 还是 0） */
    fun setIdentity(info: DeviceInfo) { identity = info }

    fun start(fromPort: Int): Int {
        var p = fromPort
        while (p < fromPort + 20) {
            try {
                server = ServerSocket(p)
                running = true
                Thread { acceptLoop() }.apply { isDaemon = true }.start()
                return p
            } catch (_: Exception) { p++ }
        }
        error("HTTP 服务启动失败")
    }

    fun stop() {
        running = false
        runCatching { server?.close() }
    }

    private fun acceptLoop() {
        val srv = server ?: return
        while (running) {
            val client = try { srv.accept() } catch (_: Exception) { break }
            Thread { handle(client) }.apply { isDaemon = true }.start()
        }
    }

    private class Req(
        val method: String, val path: String,
        val headers: Map<String, String>, val body: BodyReader,
    ) {
        fun readBody(): ByteArray? = body.readAll()
    }

    private fun handle(s: Socket) {
        runCatching {
            s.soTimeout = (PREPARE_TIMEOUT_MS + 5000).toInt()
            val input = s.getInputStream()
            val reqLine = readLine(input) ?: return
            val parts = reqLine.split(" ")
            if (parts.size < 2) return
            val headers = HashMap<String, String>()
            while (true) {
                val line = readLine(input) ?: break
                if (line.isEmpty()) break
                val i = line.indexOf(':')
                if (i > 0) headers[line.substring(0, i).trim().lowercase()] =
                    line.substring(i + 1).trim()
            }
            val req = Req(parts[0], parts[1], headers, BodyReader(input, headers))
            val resp = route(req)
            val out = s.getOutputStream()
            val head = buildString {
                append("HTTP/1.1 ${resp.code}\r\n")
                append("Content-Length: ${resp.body.size}\r\n")
                if (resp.json) append("Content-Type: application/json\r\n")
                append("Connection: close\r\n\r\n")
            }
            out.write(head.toByteArray()); out.write(resp.body); out.flush()
        }
        runCatching { s.close() }
    }

    private fun route(req: Req): Resp = try {
        when {
            req.method == "GET" && req.path == "/api/info" ->
                Resp.okJson(identity.toJson().toString())

            req.method == "POST" && req.path == "/api/message" -> {
                val j = JSONObject(String(req.readBody() ?: return Resp.err(400, "body"),
                    Charsets.UTF_8))
                val sender = DeviceInfo.fromJson(j.getJSONObject("sender"))
                val text = j.optString("text").trim()
                if (text.isEmpty() || sender.id == identity.id) Resp.err(400, "无效消息")
                else {
                    main.post { callbacks.onMessage(sender.id, sender.name, text) }
                    Resp.ok()
                }
            }

            req.method == "POST" && req.path == "/api/transfer/prepare" ->
                prepare(req)

            req.method == "PUT" && req.path.startsWith("/api/transfer/upload/") -> {
                val seg = req.path.removePrefix("/api/transfer/upload/").split("/")
                if (seg.size != 2) Resp.err(404, "not found")
                else upload(req, seg[0], seg[1])
            }

            req.method == "POST" && req.path.startsWith("/api/transfer/cancel/") -> {
                sessions.remove(req.path.removePrefix("/api/transfer/cancel/"))
                Resp.ok()
            }

            else -> Resp.err(404, "not found")
        }
    } catch (e: Exception) {
        Resp.err(500, e.message ?: "error")
    }

    private fun prepare(req: Req): Resp {
        val j = JSONObject(String(req.readBody() ?: return Resp.err(400, "body"),
            Charsets.UTF_8))
        val sender = DeviceInfo.fromJson(j.getJSONObject("sender"))
        if (sender.id == identity.id) return Resp.err(400, "自己传自己？")
        val arr: JSONArray = j.getJSONArray("files")
        val files = (0 until arr.length()).map {
            val f = arr.getJSONObject(it)
            FileMeta(f.getString("id"), f.optString("name"),
                f.optString("rel_path"), f.optLong("size"))
        }
        if (files.isEmpty()) return Resp.err(400, "文件列表为空")
        val decision = CompletableFuture<Boolean>()
        main.post { callbacks.onIncoming(IncomingReq(sender, files, decision)) }
        val ok = try {
            decision.get(PREPARE_TIMEOUT_MS, TimeUnit.MILLISECONDS)
        } catch (_: Exception) { false }
        if (!ok) return Resp.err(403, "对方拒绝了传输")
        val token = UUID.randomUUID().toString()
        val dir = File(context.cacheDir, "LocalTransfer").apply { mkdirs() }
        sessions[token] = RecvSession(token, sender.id, files, dir)
        return Resp.okJson(JSONObject().put("token", token).toString())
    }

    private fun upload(req: Req, token: String, fileId: String): Resp {
        val sess = sessions[token] ?: return Resp.err(409, "会话不存在")
        val meta = sess.files.firstOrNull { it.id == fileId }
            ?: FileMeta(fileId, fileId, fileId, 0)
        val segs = meta.relPath.split('/').filter {
            it.isNotEmpty() && it != "." && it != ".." && !it.contains(':')
        }
        val rel = segs.joinToString("/")
        val f = File(sess.saveDir, if (segs.isEmpty()) meta.name else segs.joinToString("/"))
        f.parentFile?.mkdirs()

        // 声明大小（进度分母）：chunked 时没有 Content-Length，用 meta.size
        val declared = meta.size
        val label = if (rel.contains('/')) rel.substring(0, rel.indexOf('/')) else rel
        var written = 0L
        val buf = ByteArray(64 * 1024)
        val out = f.outputStream()
        try {
            while (true) {
                val n = req.body.read(buf)
                if (n <= 0) break
                out.write(buf, 0, n)
                written += n
                val w = written
                main.post {
                    callbacks.onProgress(token, RecvProgress(
                        sess.peerId, label, sess.completed, sess.files.size,
                        w, declared))
                }
            }
        } finally { runCatching { out.flush(); out.close() } }

        // 定长模式校验大小；chunked 以实际收到为准（>0 即有效）
        val hasLen = req.headers.containsKey("content-length")
        if (hasLen && written != declared) return Resp.err(400, "大小不符")
        if (written == 0L && declared > 0) return Resp.err(400, "空内容")

        val (uri, path) = publishToDownloads(f, rel)
        // 转存失败（uri 和 path 都空）→ 用缓存路径兜底（至少能打开）
        val (finalUri, finalPath) = if (uri == null && path == null) {
            null to f.absolutePath
        } else {
            uri to path
        }
        sess.done.add(ReceivedFile(
            if (rel.contains('/')) rel.substring(rel.indexOf('/') + 1) else rel,
            written, finalUri, finalPath))
        sess.completed++
        if (sess.completed >= sess.files.size) {
            sessions.remove(token)
            val files = sess.done.toList()
            main.post { callbacks.onBatchDone(sess.peerId, files) }
        }
        return Resp.ok()
    }

    /** 复制进公共下载目录。三级回退：
     *  1) MediaStore + 子目录（Download/LocalTransfer/…）——部分 ROM 不自动建目录
     *  2) MediaStore 平铺（Download/ 根 + "LT_" 前缀保留层级信息）
     *  3) 应用外部专项目录（Android/data/<pkg>/files/Download/LocalTransfer）——
     *     永远可写、USB 可见；卡片如实标注位置 */
    private fun publishToDownloads(src: File, rel: String): Pair<String?, String?> {
        val segs = rel.split('/').filter { it.isNotEmpty() && it != ".." }
        val display = segs.lastOrNull() ?: src.name
        val sub = if (segs.size > 1) segs.dropLast(1).joinToString("/") else ""

        if (Build.VERSION.SDK_INT >= 29) {
            // 尝试 1：带子目录
            try {
                val uri = mediaStoreInsert(src, display,
                    "Download/LocalTransfer" + if (sub.isNotEmpty()) "/$sub" else "")
                if (uri != null) return uri
            } catch (e: Exception) {
                android.util.Log.w("LocalTransfer", "MediaStore 子目录失败: ${e.message}")
            }
            // 尝试 2：平铺（文件名带层级前缀）
            val flatName = ("LocalTransfer_" +
                    (if (sub.isNotEmpty()) sub.replace('/', '_') + "_" else "")) + display
            try {
                val uri = mediaStoreInsert(src, flatName, "Download")
                if (uri != null) return uri
            } catch (e: Exception) {
                android.util.Log.w("LocalTransfer", "MediaStore 平铺失败: ${e.message}")
            }
        } else {
            // API < 29：直接文件路径
            try {
                @Suppress("DEPRECATION")
                val dir = File(Environment.getExternalStoragePublicDirectory(
                    Environment.DIRECTORY_DOWNLOADS), "LocalTransfer" +
                    if (sub.isNotEmpty()) "/$sub" else "")
                dir.mkdirs()
                val dst = File(dir, display)
                src.copyTo(dst, overwrite = true); src.delete()
                publishError = null
                return null to dst.absolutePath
            } catch (e: Exception) {
                android.util.Log.w("LocalTransfer", "直写失败: ${e.message}")
            }
        }

        // 尝试 3：应用外部专项目录（永远可写）
        return try {
            val dir = File(context.getExternalFilesDir(
                Environment.DIRECTORY_DOWNLOADS), "LocalTransfer")
            dir.mkdirs()
            val dst = File(dir, display)
            src.copyTo(dst, overwrite = true); src.delete()
            publishError = "已存到应用目录 Android/data/${context.packageName}/files/Download/LocalTransfer"
            null to dst.absolutePath
        } catch (e: Exception) {
            android.util.Log.w("LocalTransfer", "转存全部失败: ${e.message}", e)
            publishError = e.message?.take(60) ?: e.javaClass.simpleName
            null to null
        }
    }

    /** MediaStore 插入 + 写入 + 清 pending；失败返回 null（不抛） */
    private fun mediaStoreInsert(src: File, display: String, relPath: String): Pair<String?, String?>? {
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, display)
            put(MediaStore.MediaColumns.MIME_TYPE, "application/octet-stream")
            put(MediaStore.Downloads.RELATIVE_PATH, relPath)
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = context.contentResolver.insert(
            MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
            ?: return null
        context.contentResolver.openOutputStream(uri)?.use { o ->
            src.inputStream().use { it.copyTo(o) }
        } ?: return null
        context.contentResolver.update(uri, ContentValues().apply {
            put(MediaStore.MediaColumns.IS_PENDING, 0)
        }, null, null)
        val p = context.contentResolver.query(uri,
            arrayOf(MediaStore.MediaColumns.DATA), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null }
        src.delete()
        publishError = null
        return uri.toString() to p
    }

    /** 最近一次转存的结果说明（卡片标注用；null = 正常 Download/LocalTransfer） */
    @Volatile var publishError: String? = null
}

private fun readLine(input: InputStream): String? {
    val sb = StringBuilder()
    while (true) {
        val b = input.read()
        if (b < 0) return if (sb.isEmpty()) null else sb.toString()
        if (b == '\n'.code) break
        if (b != '\r'.code) sb.append(b.toChar())
        if (sb.length > 16 * 1024) return null
    }
    return sb.toString()
}
