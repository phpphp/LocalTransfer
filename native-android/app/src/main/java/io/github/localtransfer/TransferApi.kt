package io.github.localtransfer

import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.OutputStream
import java.net.HttpURLConnection
import java.net.URL

/** 业务错误（message 即用户提示文案） */
class TransferException(message: String) : Exception(message)

/** 桌面端 HTTP API 客户端：发文本、prepare → 逐文件流式上传（带进度） */
class TransferApi(private val me: DeviceInfo) {

    private fun base(p: Peer) = "http://${p.addr.hostAddress}:${p.info.port}"

    fun sendText(peer: Peer, text: String) {
        val conn = post("${base(peer)}/api/message")
        val body = JSONObject()
            .put("sender", me.toJson())
            .put("text", text)
            .put("sent_at", System.currentTimeMillis())
            .toString().toByteArray()
        conn.outputStream.use { it.write(body) }
        val code = conn.responseCode
        conn.disconnect()
        if (code != 200) throw TransferException("发送失败（$code）")
    }

    /**
     * 发一组文件：prepare（等确认 ≤60s）→ 逐文件流式上传。
     * 输出流先取再写（等价 Flutter 端"先 send 再填 sink"，
     * 顺序反了会反压死锁）。
     */
    fun sendFiles(
        peer: Peer,
        files: List<Pair<FileMeta, String>>, // (meta, 本地路径)
        onProgress: (idx: Int, transferred: Long, total: Long) -> Unit,
    ) {
        val metas = JSONArray().apply { files.forEach { put(it.first.toJson()) } }
        val pr = post("${base(peer)}/api/transfer/prepare")
        val prBody = JSONObject()
            .put("sender", me.toJson())
            .put("files", metas)
            .toString().toByteArray()
        pr.outputStream.use { it.write(prBody) }
        val prCode = pr.responseCode
        val prResp = (if (prCode in 200..299) pr.inputStream else pr.errorStream)
            ?.readBytes() ?: ByteArray(0)
        pr.disconnect()
        if (prCode != 200) {
            val msg = String(prResp, Charsets.UTF_8)
            throw TransferException(
                if (msg.contains("超时")) "等待确认超时：请在 5 分钟内在电脑端点「接收」"
                else if (msg.contains("拒绝")) "对方拒绝了传输"
                else "对方返回 $prCode：$msg")
        }
        val token = JSONObject(String(prResp, Charsets.UTF_8)).getString("token")

        files.forEachIndexed { i, (meta, path) ->
            val f = File(path)
            val total = f.length()
            val conn = URL("${base(peer)}/api/transfer/upload/$token/${meta.id}")
                .openConnection() as HttpURLConnection
            conn.requestMethod = "PUT"
            conn.doOutput = true
            conn.connectTimeout = 5000
            conn.readTimeout = 10 * 60_000
            conn.setRequestProperty("Content-Type", "application/octet-stream")
            conn.setFixedLengthStreamingMode(total)
            val out: OutputStream = conn.outputStream   // 先拿流（建立连接）
            f.inputStream().use { ins ->
                val buf = ByteArray(256 * 1024)
                var transferred = 0L
                while (true) {
                    val n = ins.read(buf)
                    if (n < 0) break
                    out.write(buf, 0, n)
                    transferred += n
                    onProgress(i, transferred, total)
                }
            }
            out.flush(); out.close()
            if (conn.responseCode != 200) {
                runCatching { cancel(peer, token) }
                throw TransferException("上传失败（${conn.responseCode}）")
            }
            conn.disconnect()
        }
    }

    fun cancel(peer: Peer, token: String) {
        runCatching {
            val conn = post("${base(peer)}/api/transfer/cancel/$token")
            conn.connect(); conn.disconnect()
        }
    }

    private fun post(url: String): HttpURLConnection =
        (URL(url).openConnection() as HttpURLConnection).apply {
            requestMethod = "POST"
            doOutput = true
            connectTimeout = 5000
            readTimeout = (PREPARE_TIMEOUT_MS + 5000).toInt()
            setRequestProperty("Content-Type", "application/json")
        }
}
