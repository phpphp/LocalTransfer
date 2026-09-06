package io.github.localtransfer

import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.OpenableColumns
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.lifecycleScope
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.qrcode.QRCodeWriter
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.GlobalScope
import kotlinx.coroutines.launch
import java.io.File
import java.util.UUID

class MainActivity : ComponentActivity() {

    private val scanLauncher = registerForActivityResult(ScanContract()) { result ->
        result.contents?.let { App.addManual(it) }
    }
    private val pickFiles =
        registerForActivityResult(ActivityResultContracts.GetMultipleContents()) { uris ->
            if (uris.isNotEmpty()) App.sendPicked(uris)
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        App.init(applicationContext)
        setContent { App() }
    }

    fun startScan() {
        scanLauncher.launch(ScanOptions().apply {
            setDesiredBarcodeFormats(ScanOptions.QR_CODE)
            setPrompt("对准电脑端二维码（http://IP:端口）")
            setBeepEnabled(false)
        })
    }

    fun pick() = pickFiles.launch("*/*")

    companion object {
        /** 打开接收的文件（URI 优先，失败回退真实路径） */
        fun openFile(context: Context, f: ReceivedFile) {
            try {
                val intent = Intent(Intent.ACTION_VIEW).apply {
                    val uri = f.uri?.let { Uri.parse(it) }
                        ?: f.path?.let { Uri.fromFile(File(it)) } ?: return
                    setDataAndType(uri, mimeOf(f.name))
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                }
                context.startActivity(intent)
            } catch (e: Exception) {
                toast(context, "打开失败：${e.message}")
            }
        }

        private fun mimeOf(name: String): String = when (
            name.substringAfterLast('.', "").lowercase()) {
            "jpg", "jpeg" -> "image/jpeg"
            "png" -> "image/png"
            "gif" -> "image/gif"
            "webp" -> "image/webp"
            "mp4" -> "video/mp4"
            "mp3" -> "audio/mpeg"
            "pdf" -> "application/pdf"
            "txt", "md", "log" -> "text/plain"
            else -> "application/octet-stream"
        }

        fun toast(context: Context, text: String) {
            android.widget.Toast.makeText(context, text,
                android.widget.Toast.LENGTH_SHORT).show()
        }

        fun qrBitmap(content: String, size: Int = 512): Bitmap? = runCatching {
            val matrix = QRCodeWriter().encode(content, BarcodeFormat.QR_CODE,
                size, size, mapOf(EncodeHintType.MARGIN to 1))
            val bmp = Bitmap.createBitmap(size, size, Bitmap.Config.RGB_565)
            for (y in 0 until size) for (x in 0 until size)
                bmp.setPixel(x, y, if (matrix[x, y]) Color.BLACK else Color.WHITE)
            bmp
        }.getOrNull()
    }
}

/** 全局应用状态（发现 + 服务 + 会话）；状态只在主线程写 */
object App {
    lateinit var me: DeviceInfo
    lateinit var disc: Discovery
    lateinit var server: MiniHttpServer
    lateinit var api: TransferApi
    lateinit var ctx: Context
    private var inited = false
    private val main = Handler(Looper.getMainLooper())

    val peers get() = disc.peers
    val chats = mutableStateMapOf<String, MutableList<ChatEntry>>()
    val progress = mutableStateMapOf<String, RecvProgress>()
    var pendingReq by mutableStateOf<IncomingReq?>(null)
    var sendStatus by mutableStateOf<String?>(null)
    var currentPeer by mutableStateOf<String?>(null)

    fun init(context: Context) {
        if (inited) return
        inited = true
        ctx = context.applicationContext
        val prefs = ctx.getSharedPreferences("lt", Context.MODE_PRIVATE)
        var id = prefs.getString("device_id", null)
        if (id == null) {
            id = UUID.randomUUID().toString()
            prefs.edit().putString("device_id", id).apply()
        }
        val name = prefs.getString("device_name", null)
            ?: "手机-${id.take(4)}".also { prefs.edit().putString("device_name", it).apply() }
        me = DeviceInfo(id, name, "android", 0)
        api = TransferApi(me)
        server = MiniHttpServer(me, ctx, object : MiniHttpServer.Callbacks {
            override fun onMessage(peerId: String, peerName: String, text: String) {
                chats.getOrPut(peerId) { mutableListOf() }
                    .add(ChatEntry.Text(false, text, System.currentTimeMillis()))
            }
            override fun onIncoming(req: IncomingReq) { pendingReq = req }
            override fun onProgress(token: String, p: RecvProgress) { progress[token] = p }
            override fun onBatchDone(peerId: String, files: List<ReceivedFile>) {
                progress.clear()
                chats.getOrPut(peerId) { mutableListOf() }.add(ChatEntry.FileCard(
                    false,
                    if (files.size == 1) files[0].name else "${files.size} 个文件",
                    files.sumOf { it.size },
                    System.currentTimeMillis(),
                    "Download/LocalTransfer", files))
            }
        })
        me = me.copy(port = server.start(DEFAULT_HTTP_PORT))
        disc = Discovery(me, ctx)
        disc.restoreManual(prefs.getStringSet("manual_peers", emptySet())?.toList()
            ?: emptyList())
        disc.start(me.port)
    }

    fun addManual(addr: String) {
        val ip = addr.trim().removePrefix("http://").substringBefore(':')
        if (ip.isEmpty()) return
        GlobalScope.launch(Dispatchers.IO) {
            disc.addManual(ip, persisted = true)
            main.post {
                ctx.getSharedPreferences("lt", Context.MODE_PRIVATE)
                    .edit().putStringSet("manual_peers",
                        disc.manualIpsSnapshot()).apply()
            }
        }
    }

    fun sendText(peerId: String, text: String) {
        val p = peers.value[peerId] ?: return
        chats.getOrPut(peerId) { mutableListOf() }
            .add(ChatEntry.Text(true, text, System.currentTimeMillis()))
        GlobalScope.launch(Dispatchers.IO) {
            runCatching { api.sendText(p, text) }
                .onFailure { main.post { sendStatus = it.message } }
        }
    }

    fun sendPicked(uris: List<Uri>) {
        val peerId = currentPeer ?: return
        val p = peers.value[peerId] ?: return
        GlobalScope.launch(Dispatchers.IO) {
            main.post { sendStatus = "准备发送…" }
            val metas = mutableListOf<Pair<FileMeta, String>>()
            uris.forEach { uri ->
                val name = queryName(uri)
                val dst = File(ctx.cacheDir, "${UUID.randomUUID()}_$name")
                ctx.contentResolver.openInputStream(uri)?.use { ins ->
                    dst.outputStream().use { ins.copyTo(it) }
                } ?: return@forEach
                metas.add(FileMeta(UUID.randomUUID().toString(), name, name,
                    dst.length()) to dst.path)
            }
            if (metas.isEmpty()) { main.post { sendStatus = null }; return@launch }
            main.post {
                chats.getOrPut(peerId) { mutableListOf() }.add(ChatEntry.FileCard(
                    true,
                    if (metas.size == 1) metas[0].first.name else "${metas.size} 个文件",
                    metas.sumOf { it.first.size },
                    System.currentTimeMillis(), null, emptyList()))
            }
            runCatching {
                api.sendFiles(p, metas) { i, t, total ->
                    main.post {
                        sendStatus = "发送 ${i + 1}/${metas.size}：" +
                            "${t * 100 / total.coerceAtLeast(1)}%"
                    }
                }
                main.post { sendStatus = null }
            }.onFailure { main.post { sendStatus = it.message } }
        }
    }

    private fun queryName(uri: Uri): String {
        ctx.contentResolver.query(uri, null, null, null, null)?.use { c ->
            val idx = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
            if (idx >= 0 && c.moveToFirst()) return c.getString(idx)
        }
        return uri.lastPathSegment ?: "file"
    }
}

@SuppressLint("UnusedMaterial3ScaffoldPaddingParameter")
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun App() {
    val ctx = LocalContext.current
    MaterialTheme(colorScheme = darkColorScheme(
        primary = androidx.compose.ui.graphics.Color(0xFF6366F1))) {
        val peerId = App.currentPeer
        if (peerId == null) DeviceListScreen() else ChatScreen(peerId)
        App.pendingReq?.let { req ->
            AlertDialog(
                onDismissRequest = { },
                title = { Text("${req.peer.name} 想发送文件") },
                text = { Text("${req.files.size} 个文件 · " +
                        fmtSize(req.files.sumOf { it.size })) },
                confirmButton = { TextButton(onClick = {
                    req.decision.complete(true); App.pendingReq = null
                }) { Text("接收") } },
                dismissButton = { TextButton(onClick = {
                    req.decision.complete(false); App.pendingReq = null
                }) { Text("拒绝") } },
            )
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DeviceListScreen() {
    val ctx = LocalContext.current
    val activity = ctx as? MainActivity
    val peers by App.peers.collectAsState()
    val online = peers.values.filter {
        it.manual || System.currentTimeMillis() - it.lastSeen < DEVICE_TIMEOUT_MS
    }.sortedBy { it.info.name }
    Scaffold(
        topBar = {
            TopAppBar(title = { Text(App.me.name) }, actions = {
                IconButton(onClick = { activity?.startScan() }) {
                    Text("＋", fontSize = 22.sp)
                }
            })
        },
    ) { pad ->
        Column(Modifier.padding(pad).fillMaxSize()) {
            if (online.isEmpty()) {
                Box(Modifier.weight(1f).fillMaxWidth(),
                    contentAlignment = Alignment.Center) {
                    Text("等待设备上线…",
                        color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            } else {
                LazyColumn(Modifier.weight(1f)) {
                    items(online, key = { it.info.id }) { p ->
                        ListItem(
                            headlineContent = { Text(p.info.name) },
                            supportingContent = {
                                Text("${p.info.plat} · ${p.addr.hostAddress}:" +
                                        p.info.port, fontSize = 12.sp)
                            },
                            modifier = Modifier.clickable {
                                App.currentPeer = p.info.id
                            },
                        )
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChatScreen(peerId: String) {
    val ctx = LocalContext.current
    val activity = ctx as? MainActivity
    val peers by App.peers.collectAsState()
    val peer = peers[peerId]
    val msgs = remember(peerId) { App.chats.getOrPut(peerId) { mutableListOf() } }
    var input by remember { mutableStateOf("") }
    val progresses = App.progress.values.filter { it.peerId == peerId }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(peer?.info?.name ?: "设备", fontSize = 16.sp)
                        Text(if (peer != null) "在线" else "离线", fontSize = 12.sp,
                            color = if (peer != null) MaterialTheme.colorScheme.primary
                            else MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                },
                navigationIcon = {
                    TextButton(onClick = { App.currentPeer = null }) { Text("←") }
                },
            )
        },
    ) { pad ->
        Column(Modifier.padding(pad).fillMaxSize().imePadding()) {
            App.sendStatus?.let {
                Text(it, fontSize = 12.sp,
                    modifier = Modifier.padding(horizontal = 12.dp))
            }
            LazyColumn(Modifier.weight(1f).padding(8.dp)) {
                items(msgs.size + progresses.size) { i ->
                    if (i < msgs.size) MessageBubble(msgs[i])
                    else ProgressCard(progresses[i - msgs.size])
                }
            }
            Row(Modifier.padding(8.dp), verticalAlignment = Alignment.CenterVertically) {
                OutlinedButton(onClick = { activity?.pick() }) { Text("📎") }
                Spacer(Modifier.width(8.dp))
                OutlinedTextField(input, { input = it }, Modifier.weight(1f),
                    placeholder = { Text("输入消息") }, maxLines = 4)
                Spacer(Modifier.width(8.dp))
                Button(onClick = {
                    if (input.isNotBlank()) { App.sendText(peerId, input); input = "" }
                }) { Text("发送") }
            }
        }
    }
}

@Composable
fun MessageBubble(e: ChatEntry) {
    val ctx = LocalContext.current
    val end = e.outgoing
    Box(Modifier.fillMaxWidth().padding(vertical = 4.dp),
        contentAlignment = if (end) Alignment.CenterEnd else Alignment.CenterStart) {
        Surface(
            shape = RoundedCornerShape(14.dp),
            color = if (end) MaterialTheme.colorScheme.primary
            else MaterialTheme.colorScheme.surfaceVariant,
        ) {
            Column(Modifier.padding(10.dp).widthIn(max = 280.dp)) {
                when (e) {
                    is ChatEntry.Text -> Text(e.text,
                        color = if (end) MaterialTheme.colorScheme.onPrimary
                        else MaterialTheme.colorScheme.onSurface)
                    is ChatEntry.FileCard -> Column(
                        Modifier.clickable {
                            e.files.firstOrNull()?.let {
                                MainActivity.openFile(ctx, it)
                            }
                        }) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(if (e.files.size > 1 || e.files.isEmpty()) "📁" else "📄")
                            Spacer(Modifier.width(6.dp))
                            Text(e.title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                        e.location?.let {
                            Text(it, fontSize = 10.sp,
                                color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        Text("${fmtSize(e.size)} · 点击打开", fontSize = 11.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    is ChatEntry.SendProgress -> {}
                }
            }
        }
    }
}

@Composable
fun ProgressCard(p: RecvProgress) {
    Surface(shape = RoundedCornerShape(14.dp),
        color = MaterialTheme.colorScheme.surfaceVariant) {
        Column(Modifier.padding(10.dp).widthIn(max = 280.dp)) {
            Row {
                Text("📁 ${p.label}")
                Text("（${p.fileIdx + 1}/${p.fileCount}）", fontSize = 11.sp)
            }
            val frac = if (p.total > 0) p.transferred.toFloat() / p.total else 0f
            Text("接收中 ${(frac * 100).toInt()}% · " +
                    "${fmtSize(p.transferred)} / ${fmtSize(p.total)}",
                fontSize = 11.sp,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(6.dp))
            LinearProgressIndicator(progress = { frac },
                modifier = Modifier.fillMaxWidth())
        }
    }
}
