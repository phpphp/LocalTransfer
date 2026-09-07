package io.github.localtransfer

import android.content.Context
import android.net.wifi.WifiManager
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.joinAll
import kotlinx.coroutines.launch
import kotlinx.coroutines.coroutineScope
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

    /** 诊断状态（设备列表底部小字显示，排查"互相看不见"） */
    val udpStatus = MutableStateFlow("初始化…")
    @Volatile var rxPackets = 0
        private set

    fun start(port: Int) {
        // Android WiFi 驱动默认过滤多播帧，必须持锁
        try {
            lock = wifi.createMulticastLock("localtransfer").apply {
                setReferenceCounted(false); acquire()
            }
        } catch (_: Exception) {}
        // 各循环独立协程——之前 loop 排在 announceLoop 前面顺序执行，
        // loop 的 receive() 永远阻塞 → announce 从未发过（手机对电脑不可见的根因）
        scope.launch { bindAndLoop() }
        scope.launch { while (true) { announce(); delay(5000) } }
        scope.launch { while (true) { runCatching { subnetScan() }; delay(45_000) } }
        scope.launch { while (true) { runCatching { keepalive() }; delay(6000) } }
    }

    // MARK: UDP 收发

    private fun bindAndLoop() {
        val s = try {
            // reuseAddress 必须在 bind 之前：未绑定构造 → 设选项 → bind。
            // bind/join 的任何异常都不吞——发不出去 announce 手机就对电脑不可见
            val tmp = MulticastSocket(null as java.net.SocketAddress?)
            runCatching { tmp.reuseAddress = true }
            tmp.broadcast = true
            tmp.bind(java.net.InetSocketAddress(DISCOVERY_PORT))
            @Suppress("DEPRECATION")
            runCatching { tmp.joinGroup(InetAddress.getByName(DISCOVERY_GROUP)) }
            udpStatus.value = "UDP √ :${DISCOVERY_PORT}"
            tmp
        } catch (e: Exception) {
            udpStatus.value = "UDP ×（${e.message}）——仅靠扫描发现"
            null
        } ?: return
        sock = s
        val buf = ByteArray(2048)
        while (true) {
            val pkt = DatagramPacket(buf, buf.size)
            try {
                s.receive(pkt)
            } catch (_: Exception) {
                continue
            }
            rxPackets++
            val raw = String(pkt.data, 0, pkt.length, Charsets.UTF_8)
            val j = runCatching { JSONObject(raw) }.getOrNull() ?: continue
            handle(j, pkt.address, s)
        }
    }

    private fun handle(j: JSONObject, from: InetAddress, s: MulticastSocket) {
        when (j.optString("t")) {
            "announce" -> {
                val info = runCatching { DeviceInfo.fromJson(j) }.getOrNull() ?: return
                if (info.id == me.id || info.v != PROTOCOL_VERSION) return
                val manual = manualIps.contains(from.hostAddress) ||
                        _peers.value[info.id]?.manual == true
                _peers.value = _peers.value +
                        (info.id to Peer(info, from, manual))
                // 单播回（2s 节流，防互回风暴）
                val now = System.currentTimeMillis()
                if (now - (lastUnicastReply[info.id] ?: 0L) >= 2000) {
                    lastUnicastReply[info.id] = now
                    val data = me.toAnnounce()
                    runCatching {
                        s.send(DatagramPacket(data, data.size, from, DISCOVERY_PORT))
                    }
                }
            }
            "bye" -> {
                val id = j.optString("id")
                if (id.isNotEmpty()) _peers.value = _peers.value - id
            }
        }
    }

    private fun announce() {
        val s = sock ?: return
        val data = me.toAnnounce()
        // 多播
        runCatching {
            s.send(DatagramPacket(data, data.size,
                InetAddress.getByName(DISCOVERY_GROUP), DISCOVERY_PORT))
        }
        // 广播兜底
        runCatching {
            s.send(DatagramPacket(data, data.size,
                InetAddress.getByName("255.255.255.255"), DISCOVERY_PORT))
        }
    }

    // MARK: TCP 兜底

    /** 网段扫描：探测 /24 上 17878/17879 的 /api/info */
    private suspend fun subnetScan() = coroutineScope {
        val own = myLanIp() ?: return@coroutineScope
        val dot = own.lastIndexOf('.')
        if (dot < 0) return@coroutineScope
        val prefix = own.substring(0, dot)
        (1..254).map { i ->
            launch(Dispatchers.IO) {
                httpProbe("$prefix.$i", DEFAULT_HTTP_PORT, addNew = true)
                httpProbe("$prefix.$i", DEFAULT_HTTP_PORT + 1, addNew = true)
            }
        }.joinAll()
    }

    private fun keepalive() {
        _peers.value.values.forEach { p ->
            httpProbe(p.addr.hostAddress ?: return@forEach, p.info.port, addNew = false)
        }
    }

    /** HTTP 探测 /api/info；addNew=true 时未知设备也入表；否则只刷新已知设备 */
    private fun httpProbe(ip: String, port: Int, addNew: Boolean): Boolean {
        return try {
            val conn = java.net.URL("http://$ip:$port/api/info")
                .openConnection() as java.net.HttpURLConnection
            conn.connectTimeout = 800
            conn.readTimeout = 800
            if (conn.responseCode != 200) { conn.disconnect(); return false }
            val info = DeviceInfo.fromJson(
                JSONObject(conn.inputStream.readBytes().toString(Charsets.UTF_8)))
            conn.disconnect()
            if (info.id == me.id) return false
            val cur = _peers.value
            if (!addNew && !cur.containsKey(info.id)) return false
            val manual = manualIps.contains(ip) || cur[info.id]?.manual == true
            _peers.value = cur + (info.id to
                    Peer(info, InetAddress.getByName(ip), manual))
            true
        } catch (_: Exception) { false }
    }

    /** 手动添加（persisted=true 时记入持久化集合） */
    fun addManual(ip: String, persisted: Boolean) {
        manualIps += ip
        httpProbe(ip, DEFAULT_HTTP_PORT, addNew = true)
        httpProbe(ip, DEFAULT_HTTP_PORT + 1, addNew = true)
    }

    fun restoreManual(list: List<String>) { manualIps.addAll(list) }
    fun manualIpsSnapshot(): Set<String> = manualIps.toSet()

    fun shutdown() {
        runCatching {
            sock?.let { s ->
                val bye = JSONObject().put("t", "bye").put("id", me.id)
                    .toString().toByteArray()
                s.send(DatagramPacket(bye, bye.size,
                    InetAddress.getByName(DISCOVERY_GROUP), DISCOVERY_PORT))
                s.send(DatagramPacket(bye, bye.size,
                    InetAddress.getByName("255.255.255.255"), DISCOVERY_PORT))
            }
        }
        scope.cancel()
        runCatching { sock?.close() }
        lock?.release()
    }

    companion object {
        /** getifaddrs 语义：第一个非环回 IPv4（优先私网段） */
        fun myLanIp(): String? = runCatching {
            NetworkInterface.getNetworkInterfaces().asSequence()
                .filter { it.isUp && !it.isLoopback }
                .flatMap { it.inetAddresses.asSequence() }
                .filter { it is java.net.Inet4Address }
                .map { it.hostAddress!! }
                .firstOrNull { it.startsWith("192.168.") || it.startsWith("10.") }
                ?: NetworkInterface.getNetworkInterfaces().asSequence()
                    .filter { it.isUp && !it.isLoopback }
                    .flatMap { it.inetAddresses.asSequence() }
                    .filter { it is java.net.Inet4Address && !it.isLoopbackAddress }
                    .firstOrNull()?.hostAddress
        }.getOrNull()
    }
}
