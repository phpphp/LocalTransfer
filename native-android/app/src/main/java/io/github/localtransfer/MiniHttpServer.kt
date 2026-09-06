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
 * 极简 HTTP/1.1 服务（路由与桌面端 axum 对齐）。
 * 手写解析：请求行 + 头 + Content-Length 请求体；连接一问一答后关闭。
 * 回调统一切到主线程（Compose 状态只能在主线程写）。
 */
class MiniHttpServer(
    private val me: DeviceInfo,
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

    val port: Int get() = server?.localPort ?: 0

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
        val headers: Map<String, String>, val body: InputStream,
    )

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
            val req = Req(parts[0], parts[1], headers, input)
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
                Resp.okJson(me.toJson().toString())

            req.method == "POST" && req.path == "/api/message" -> {
                val j = JSONObject(req.body.readBytes().toString(Charsets.UTF_8))
                val sender = DeviceInfo.fromJson(j.getJSONObject("sender"))
                val text = j.optString("text").trim()
                if (text.isEmpty() || sender.id == me.id) Resp.err(400, "无效消息")
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
        val j = JSONObject(req.body.readBytes().toString(Charsets.UTF_8))
        val sender = DeviceInfo.fromJson(j.getJSONObject("sender"))
        if (sender.id == me.id) return Resp.err(400, "自己传自己？")
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

        val len = req.headers["content-length"]?.toLongOrNull() ?: 0
        val label = if (rel.contains('/')) rel.substring(0, rel.indexOf('/')) else rel
        var written = 0L
        val buf = ByteArray(64 * 1024)
        val out = f.outputStream()
        try {
            var remain = len
            while (remain > 0) {
                val n = req.body.read(buf, 0, minOf(buf.size.toLong(), remain).toInt())
                if (n <= 0) break
                out.write(buf, 0, n); written += n; remain -= n
                val w = written
                main.post {
                    callbacks.onProgress(token, RecvProgress(
                        sess.peerId, label, sess.completed, sess.files.size, w, len))
                }
            }
        } finally { runCatching { out.flush(); out.close() } }

        if (written != len) return Resp.err(400, "大小不符")

        val (uri, path) = publishToDownloads(f, rel)
        sess.done.add(ReceivedFile(
            if (rel.contains('/')) rel.substring(rel.indexOf('/') + 1) else rel,
            written, uri, path))
        sess.completed++
        if (sess.completed >= sess.files.size) {
            sessions.remove(token)
            val files = sess.done.toList()
            main.post { callbacks.onBatchDone(sess.peerId, files) }
        }
        return Resp.ok()
    }

    /** 复制进公共下载目录（Download/LocalTransfer/...），返回 (uri, 真实路径) */
    private fun publishToDownloads(src: File, rel: String): Pair<String?, String?> {
        val segs = rel.split('/').filter { it.isNotEmpty() && it != ".." }
        val display = segs.lastOrNull() ?: src.name
        val sub = if (segs.size > 1) segs.dropLast(1).joinToString("/") else ""
        return try {
            if (Build.VERSION.SDK_INT >= 29) {
                val values = ContentValues().apply {
                    put(MediaStore.Downloads.DISPLAY_NAME, display)
                    put(MediaStore.MediaColumns.MIME_TYPE, "application/octet-stream")
                    put(MediaStore.Downloads.RELATIVE_PATH,
                        "Download/LocalTransfer" + if (sub.isNotEmpty()) "/$sub" else "")
                }
                val uri = context.contentResolver.insert(
                    MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
                    ?: return null to null
                context.contentResolver.openOutputStream(uri)?.use { o ->
                    src.inputStream().use { it.copyTo(o) }
                }
                val p = context.contentResolver.query(uri,
                    arrayOf(MediaStore.MediaColumns.DATA), null, null, null)?.use { c ->
                    if (c.moveToFirst()) c.getString(0) else null }
                src.delete()
                uri.toString() to p
            } else {
                @Suppress("DEPRECATION")
                val dir = File(Environment.getExternalStoragePublicDirectory(
                    Environment.DIRECTORY_DOWNLOADS), "LocalTransfer" +
                    if (sub.isNotEmpty()) "/$sub" else "")
                dir.mkdirs()
                val dst = File(dir, display)
                src.copyTo(dst, overwrite = true); src.delete()
                null to dst.absolutePath
            }
        } catch (_: Exception) { null to null }
    }
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
