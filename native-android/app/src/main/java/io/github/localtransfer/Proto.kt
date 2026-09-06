package io.github.localtransfer

import org.json.JSONObject

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
