package io.github.localtransfer

import org.json.JSONObject
import java.util.UUID

const val PROTOCOL_VERSION = 1
const val DISCOVERY_GROUP = "239.192.71.82"
const val DISCOVERY_PORT = 17878
const val DEFAULT_HTTP_PORT = 17878
const val DEVICE_TIMEOUT_MS = 15_000L
const val PREPARE_TIMEOUT_MS = 60_000L

data class DeviceInfo(
    val id: String,
    val name: String,
    val plat: String,
    val port: Int,
    val v: Int = PROTOCOL_VERSION,
) {
    fun toJson(): JSONObject = JSONObject()
        .put("id", id).put("name", name).put("plat", plat).put("port", port).put("v", v)

    fun toAnnounce(): ByteArray = JSONObject()
        .put("t", "announce")
        .put("id", id).put("name", name).put("plat", plat).put("port", port).put("v", v)
        .toString().toByteArray()

    companion object {
        fun fromJson(j: JSONObject) = DeviceInfo(
            id = j.getString("id"),
            name = j.optString("name", "?"),
            plat = j.optString("plat", "unknown"),
            port = j.optInt("port", DEFAULT_HTTP_PORT),
            v = j.optInt("v", 0),
        )
    }
}

data class FileMeta(val id: String, val name: String, val relPath: String, val size: Long) {
    fun toJson(): JSONObject = JSONObject()
        .put("id", id).put("name", name).put("rel_path", relPath).put("size", size)
}

sealed class ChatEntry {
    data class Text(val outgoing: Boolean, val text: String, val atMs: Long) : ChatEntry()
    data class FileCard(
        val outgoing: Boolean,
        val title: String,
        val size: Long,
        val atMs: Long,
        val location: String?,
        val files: List<ReceivedFile>,
    ) : ChatEntry()
    data class SendProgress(val label: String, val idx: Int, val count: Int,
                            val transferred: Long, val total: Long) : ChatEntry()
}

/** 接收批次里的一个文件（打开用） */
data class ReceivedFile(
    val name: String,
    val size: Long,
    val uri: String?,      // content://（MediaStore）
    val path: String?,     // 真实路径（回退）
)

data class RecvProgress(
    val peerId: String, val label: String,
    val fileIdx: Int, val fileCount: Int,
    val transferred: Long, val total: Long,
    /** 瞬时速度（字节/秒），0=未知 */
    val speedBps: Double = 0.0,
    /** 方向标记：true=发送中（进度卡文案用） */
    val sending: Boolean = false,
)

/** 消息是否自己发出的（气泡朝向） */
val ChatEntry.outgoing: Boolean
    get() = when (this) {
        is ChatEntry.Text -> outgoing
        is ChatEntry.FileCard -> outgoing
        is ChatEntry.SendProgress -> true
    }

fun fmtSize(bytes: Long): String {
    val u = arrayOf("B", "KB", "MB", "GB", "TB")
    var v = bytes.toDouble(); var i = 0
    while (v >= 1024 && i < 4) { v /= 1024; i++ }
    return if (i == 0) "$bytes B" else String.format("%.1f %s", v, u[i])
}

// ---------------------------------------------------------------- 随机诗意名（与桌面端 store.rs 同词表）

private val POETIC_ADJ = arrayOf(
    "温柔的", "安静的", "快乐的", "勇敢的", "自由的", "神秘的", "优雅的", "活泼的",
    "沉静的", "明亮的", "柔软的", "轻盈的", "悠然的", "清澈的", "可爱的", "狡黠的",
    "坦率的", "浪漫的", "顽皮的", "认真的", "热烈的", "朦胧的", "顺风的", "发光的",
    "微笑的", "好奇的", "懒洋洋的", "慢悠悠的", "亮晶晶的", "毛茸茸的", "圆滚滚的", "慢半拍的",
)
private val POETIC_NOUN = arrayOf(
    "山雀", "鲸鱼", "萤火", "松鼠", "云雀", "海豚", "月光", "星河", "芦苇", "清泉",
    "晚风", "候鸟", "竹叶", "雪花", "灯塔", "小熊", "旅人", "橘猫", "白鹭", "远山",
    "湖泊", "松果", "蒲公英", "布谷鸟", "小雨滴", "贝壳", "枫叶", "流星", "麦浪",
    "溪水", "云朵", "海风",
)

/** 随机诗意设备名（形容词 + 的 + 自然意象，与桌面端同词表同逻辑） */
fun randomPoeticName(): String {
    val b = UUID.randomUUID().toString().replace("-", "")
    val adjIdx = b.substring(0, 2).toInt(16) % POETIC_ADJ.size
    val nounIdx = b.substring(2, 4).toInt(16) % POETIC_NOUN.size
    return POETIC_ADJ[adjIdx] + POETIC_NOUN[nounIdx]
}
