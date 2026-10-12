package io.github.localtransfer

import android.content.Context
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
     * 发一组文件：prepare（等确认 ≤5 分钟）→ 逐文件流式上传。
     * 输出流先取再写（等价 Flutter 端"先 send 再填 sink"，
     * 顺序反了会反压死锁）。
     * 返回 false = 对方 5 分钟没点确认（等待确认超时，静默取消不提示）；
     * 其余失败照旧抛 [TransferException]。
     */
    fun sendFiles(
        peer: Peer,
        files: List<Pair<FileMeta, String>>, // (meta, 本地路径)
        onProgress: (idx: Int, transferred: Long, total: Long) -> Unit,
    ): Boolean {
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
            // 等待确认超时 ≠ 拒绝：对方没点接收往往就是不想收，静默收场
            if (msg.contains("超时")) return false
            throw TransferException(
                if (msg.contains("拒绝")) "对方拒绝了传输"
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
            // 大文件慢网可能传几十分钟——不设读超时（0=无限），发送方取消/断连收场
            conn.readTimeout = 0
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
        return true
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
    /** 默认网关（/proc/net/route 的 gateway 列，十六进制小端） */
    fun defaultGateway(): String? = runCatching {
        val ws = Regex("\\s+")
        java.io.File("/proc/net/route").readLines()
            .drop(1)
            .filter { it.trim().isNotEmpty() }
            .firstOrNull { line -> ws.split(line.trim()).let {
                it.size > 2 && it[1] == "00000000" && it[2] != "00000000"
            } }
            ?.trim()?.let { ws.split(it).get(2) }
            ?.let { hex ->
                // 小端十六进制 → 点分十进制
                val ip = hex.padStart(8, '0')
                "${ip.substring(6,8).toInt(16)}.${ip.substring(4,6).toInt(16)}." +
                "${ip.substring(2,4).toInt(16)}.${ip.substring(0,2).toInt(16)}"
            }
    }.getOrNull()

    /** WiFi 链路信息（与桌面同语义）：协商速率 Mbps + 信号 0-100 + SSID。
     *  与 ping 延迟不同——速率是吞吐天花板（iperf3 类判断依据） */
    fun wifiLinkInfo(): Triple<String, Int, Int>? = runCatching {
        val ctx = App.ctx
        val wifi = ctx.getSystemService(Context.WIFI_SERVICE) as? android.net.wifi.WifiManager
            ?: return null
        @Suppress("DEPRECATION")
        val info = wifi.connectionInfo ?: return null
        if (info.ssid == "<unknown ssid>" && info.linkSpeed <= 0) return null
        val ssid = info.ssid.removeSurrounding("\"")
        // rssi → 百分比（-50≈100%，-100≈0%，与桌面同口径）
        val pct = ((2.0 * (info.rssi + 100)).toInt()).coerceIn(0, 100)
        Triple(if (ssid.isBlank()) "WiFi" else ssid, info.linkSpeed, pct)
    }.getOrNull()

    /** 网关 ping 链路质量：平均延迟 ms / 丢包%（用系统 ping） */
    fun gatewayPing(gw: String): Pair<Double, Double>? = runCatching {
        val p = ProcessBuilder("ping", "-c", "15", "-i", "0.2", gw)
            .redirectErrorStream(true).start()
        val out = p.inputStream.bufferedReader().readText()
        p.waitFor()
        // rtt min/avg/max/mdev = 1.2/3.4/... ms
        val avg = Regex("= [\\d.]+/([\\d.]+)/").find(out)?.groupValues?.get(1)?.toDoubleOrNull()
        // 15 packets transmitted, 15 received, 0% packet loss
        val loss = Regex("(\\d+)% packet loss").find(out)?.groupValues?.get(1)?.toDoubleOrNull()
        if (avg == null || loss == null) null else avg to loss
    }.getOrNull()

    /** 与对端测速：上行 POST 8MB + 下行 GET 8MB，返回 (上行 MB/s, 下行 MB/s) */
    fun speedTest(peer: Peer): Pair<Double, Double>? {
        val n = 8L * 1024 * 1024
        return runCatching {
            // 上行
            val up = URL("${base(peer)}/api/speedtest").run {
                val c = openConnection() as HttpURLConnection
                c.requestMethod = "POST"
                c.doOutput = true
                c.connectTimeout = 3000
                c.readTimeout = 10000
                c.setFixedLengthStreamingMode(n)
                val t0 = System.currentTimeMillis()
                val zeros = ByteArray(1024 * 1024)
                c.outputStream.use { o ->
                    repeat((n / zeros.size).toInt()) { o.write(zeros) }
                }
                if (c.responseCode != 200) return null
                val upSec = (System.currentTimeMillis() - t0) / 1000.0
                n / 1048576.0 / upSec
            }
            // 下行
            val down = URL("${base(peer)}/api/speedtest?size=$n").run {
                val c = openConnection() as HttpURLConnection
                c.connectTimeout = 3000
                c.readTimeout = 10000
                if (c.responseCode != 200) return null
                val t0 = System.currentTimeMillis()
                val buf = ByteArray(256 * 1024)
                var got = 0L
                c.inputStream.use { i ->
                    while (true) { val r = i.read(buf); if (r <= 0) break; got += r }
                }
                val downSec = (System.currentTimeMillis() - t0) / 1000.0
                if (got <= 0) return null
                got / 1048576.0 / downSec
            }
            up to down
        }.getOrNull()
    }

}
