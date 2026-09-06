package io.github.localtransfer

import android.content.Context
import android.net.wifi.WifiManager
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONObject
import java.net.DatagramPacket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.MulticastSocket
import java.net.NetworkInterface
import java.util.concurrent.ConcurrentHashMap

data class Peer(
    val info: DeviceInfo,
    val addr: InetAddress,
    val manual: Boolean = false,
    val lastSeen: Long = System.currentTimeMillis(),
)

/**
 * 设备发现（与桌面端/Rust、Flutter 版同协议）：
 * 1. UDP 多播 + 广播 announce（心跳 5s）+ 收到对方 announce 时单播回（2s 节流）
 * 2. TCP 网段扫描（启动 + 每 45s）：UDP 被路由器拦截时的自动发现兜底
 * 3. TCP 保活（6s）：刷新已知设备在线状态
 */
class Discovery(private val me: DeviceInfo, context: Context) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val _peers = MutableStateFlow<Map<String, Peer>>(emptyMap())
    val peers = _peers.asStateFlow()

    @Volatile private var sock: MulticastSocket? = null
    private val lastUnicastReply = ConcurrentHashMap<String, Long>()
    private val manualIps: MutableSet<String> = ConcurrentHashMap.newKeySet()

    private val wifi =
        context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
    private var lock: WifiManager.MulticastLock? = null

    fun start(port: Int) {
        // Android WiFi 驱动默认过滤多播帧，必须持锁
        try {
            lock = wifi.createMulticastLock("localtransfer").apply {
                setReferenceCounted(false); acquire()
            }
        } catch (_: Exception) {}
        scope.launch {
            runCatching { loop(port) }
            runCatching { announceLoop() }
        }
        scope.launch { subnetScanLoop() }
        scope.launch { keepaliveLoop() }
    }

    private fun loop(port: Int) {
        val s = MulticastSocket(DISCOVERY_PORT)
        s.reuseAddress = true
        runCatching { s.joinGroup(InetSocketAddress(InetAddress.getByName(DISCOVERY_GROUP), 0), null) }
        sock = s
        val buf = ByteArray(2048)
        while (true) {
            val pkt = DatagramPacket(buf, buf.size)
            s.receive(pkt)
            val raw = String(pkt.data, 0, pkt.length, Charsets.UTF_8)
            val j = runCatching { JSONObject(raw) }.getOrNull() ?: continue
            when (j.optString("t")) {
                "announce" -> {
                    val info = runCatching { DeviceInfo.fromJson(j) }.getOrNull() ?: continue
                    if (info.id == me.id || info.v != PROTOCOL_VERSION) continue
                    val existing = _peers.value[info.id]
                    _peers.value = _peers.value + (info.id to Peer(info, pkt.address))
                    // 单播回（2s 节流，防互回风暴）
                    val now = System.currentTimeMillis()
                    if (now - (lastUnicastReply[info.id] ?: 0L) >= 2000) {
                        lastUnicastReply[info.id] = now
                        runCatching {
                            s.send(DatagramPacket(me.toAnnounce(), me.toAnnounce().size,
                                pkt.address, DISCOVERY_PORT))
                        }
                    }
                    if (existing == null || existing.info != info) _peers.value = _peers.value
                }
                "bye" -> {
                    val id = j.optString("id")
                    if (id.isNotEmpty()) _peers.value = _peers.value - id
                }
            }
        }
    }

    private suspend fun announceLoop() {
        while (true) {
            sendAnnounce()
            delay(5000)
        }
    }

    private fun sendAnnounce() {
        val s = sock ?: return
        val data = me.toAnnounce()
        runCatching { s.send(DatagramPacket(data, data.size,
            InetAddress.getByName(DISCOVERY_GROUP), DISCOVERY_PORT)) }
        runCatching { s.broadcast = true; s.send(DatagramPacket(data, data.size,
            InetAddress.getByName("255.255.255.255"), DISCOVERY_PORT)) }
    }

    /** 网段扫描：探测 /24 上 17878/17879 的 /api/info */
    private suspend fun subnetScanLoop() {
        while (true) {
            runCatching { subnetScan() }
            delay(45_000)
        }
    }

    private suspend fun subnetScan() = coroutineScope {
        val own = myLanIp() ?: return@coroutineScope
        val dot = own.lastIndexOf('.')
        if (dot < 0) return@coroutineScope
        val prefix = own.substring(0, dot)
        (1..254).map { i ->
            launch(Dispatchers.IO) {
                probe("$prefix.$i", DEFAULT_HTTP_PORT, addNew = true)
                probe("$prefix.$i", DEFAULT_HTTP_PORT + 1, addNew = true)
            }
        }.joinAll()
    }

    private fun keepaliveLoop() = scope.launch {
        while (true) {
            _peers.value.values.forEach { p -> probe(p.addr.hostAddress, p.info.port, addNew = false) }
            delay(6000)
        }
    }

    /** HTTP 探测：addNew=true 时未知设备入表；否则只刷新 lastSeen */
    private fun probe(ip: String, port: Int, addNew: Boolean): Boolean =
        httpProbe(ip, port, addNew)

    fun manualIpsSnapshot(): Set<String> = manualIps.toSet()

    private fun httpProbe(ip: String, port: Int, addNew: Boolean): Boolean {
        return try {
            val conn = java.net.URL("http://$ip:$port/api/info")
                .openConnection() as java.net.HttpURLConnection
            conn.connectTimeout = 600
            conn.readTimeout = 600
            if (conn.responseCode != 200) { conn.disconnect(); return false }
            val info = DeviceInfo.fromJson(JSONObject(conn.inputStream.readBytes().toString(Charsets.UTF_8)))
            conn.disconnect()
            if (info.id == me.id) return false
            val cur = _peers.value
            if (!addNew && !cur.containsKey(info.id)) return false
            _peers.value = cur + (info.id to Peer(info, InetAddress.getByName(ip),
                manual = manualIps.contains(info.id) || cur[info.id]?.manual == true))
            true
        } catch (_: Exception) { false }
    }

    /** 手动添加（持久化集合由调用方写入 prefs） */
    fun addManual(ip: String, persisted: Boolean) {
        manualIps += ip
        httpProbe(ip, DEFAULT_HTTP_PORT, addNew = true)
    }

    fun restoreManual(list: List<String>) { manualIps.addAll(list) }

    fun shutdown() {
        runCatching {
            sock?.let { s ->
                val bye = JSONObject().put("t", "bye").put("id", me.id).toString().toByteArray()
                s.send(DatagramPacket(bye, bye.size, InetAddress.getByName(DISCOVERY_GROUP), DISCOVERY_PORT))
                s.send(DatagramPacket(bye, bye.size, InetAddress.getByName("255.255.255.255"), DISCOVERY_PORT))
            }
        }
        scope.cancel()
        runCatching { sock?.close() }
        lock?.release()
    }

    companion object {
        fun myLanIp(): String? = runCatching {
            NetworkInterface.getNetworkInterfaces().asSequence()
                .filter { it.isUp && !it.isLoopback }
                .flatMap { it.inetAddresses.asSequence() }
                .filter { it is java.net.Inet4Address }
                .map { it.hostAddress!! }
                .firstOrNull { it.startsWith("192.168.") || it.startsWith("10.") || it.startsWith("172.") }
        }.getOrNull()
    }
}
