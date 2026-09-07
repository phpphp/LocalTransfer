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
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.AttachFile
import androidx.compose.material.icons.rounded.Casino
import androidx.compose.material.icons.rounded.QrCode2
import androidx.compose.material.icons.automirrored.rounded.Send
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
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
            ?: randomPoeticName().also { prefs.edit().putString("device_name", it).apply() }
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
                // 位置标注：有 MediaStore URI（公共下载目录）才标 Download；
                // 只有缓存路径（转存失败兜底）标真实原因
                val anyPublic = files.any { it.uri != null }
                val location = if (anyPublic) "Download/LocalTransfer"
                               else "转存失败: ${server.publishError ?: "?"} · 点文件仍可打开"
                chats.getOrPut(peerId) { mutableListOf() }.add(ChatEntry.FileCard(
                    false,
                    if (files.size == 1) files[0].name else "${files.size} 个文件",
                    files.sumOf { it.size },
                    System.currentTimeMillis(),
                    location, files))
            }
        })
        me = me.copy(port = server.start(DEFAULT_HTTP_PORT))
        server.setIdentity(me)   // /api/info 返回带真实端口的身份
        disc = Discovery(me, ctx)
        disc.restoreManual(prefs.getStringSet("manual_peers", emptySet())?.toList()
            ?: emptyList())
        disc.start(me.port)
    }

    /** 改名：立即生效并持久化（下一条 announce 即携带新名） */
    fun renameDevice(newName: String) {
        val n = newName.trim()
        if (n.isEmpty()) return
        me = me.copy(name = n)
        ctx.getSharedPreferences("lt", Context.MODE_PRIVATE)
            .edit().putString("device_name", n).apply()
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

private val Indigo = androidx.compose.ui.graphics.Color(0xFF6366F1)
private val IndigoDark = androidx.compose.ui.graphics.Color(0xFF4F46E5)

@SuppressLint("UnusedMaterial3ScaffoldPaddingParameter")
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun App() {
    MaterialTheme(colorScheme = darkColorScheme(primary = Indigo)) {
        val peerId = App.currentPeer
        if (peerId == null) DeviceListScreen() else ChatScreen(peerId)
        App.pendingReq?.let { req ->
            AlertDialog(
                onDismissRequest = { },
                title = { Text("${req.peer.name} 想发送文件",
                    fontWeight = FontWeight.SemiBold) },
                text = { Text("${req.files.size} 个文件 · " +
                        fmtSize(req.files.sumOf { it.size }),
                    color = MaterialTheme.colorScheme.onSurfaceVariant) },
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

// ---------------------------------------------------------------- 设备列表

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DeviceListScreen() {
    val ctx = LocalContext.current
    val activity = ctx as? MainActivity
    val peers by App.peers.collectAsState()
    val udp by App.disc.udpStatus.collectAsState()
    val rx by remember { derivedStateOf { App.disc.rxPackets } }
    val online = peers.values.filter {
        it.manual || System.currentTimeMillis() - it.lastSeen < DEVICE_TIMEOUT_MS
    }.sortedBy { it.info.name }

    var showAdd by remember { mutableStateOf(false) }
    var showQr by remember { mutableStateOf(false) }
    var showRename by remember { mutableStateOf(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column(modifier = Modifier.clickable { showRename = true }) {
                        Text(App.me.name, fontWeight = FontWeight.SemiBold)
                        Text("LocalTransfer · 点名字改名", fontSize = 10.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                },
                actions = {
                    // 本机二维码（Material 真实二维码图标）
                    IconButton(onClick = { showQr = true }) {
                        Icon(Icons.Rounded.QrCode2, "本机二维码")
                    }
                    // 添加设备
                    IconButton(onClick = { showAdd = true }) {
                        Icon(Icons.Rounded.Add, "添加设备")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.surface),
            )
        },
    ) { pad ->
        Column(Modifier.fillMaxSize().padding(pad)) {
            if (online.isEmpty()) {
                Column(
                    Modifier.weight(1f).fillMaxWidth(),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.Center,
                ) {
                    Text("⌕", fontSize = 44.sp,
                        color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Spacer(Modifier.height(8.dp))
                    Text("等待设备上线…",
                        color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text("同一局域网自动发现 · 或点 ＋ 手动添加",
                        fontSize = 12.sp,
                        color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            } else {
                LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
                    items(online, key = { it.info.id }) { p ->
                        DeviceRow(p) { App.currentPeer = p.info.id }
                        HorizontalDivider(color = MaterialTheme.colorScheme
                            .surfaceVariant.copy(alpha = 0.4f))
                    }
                }
            }
            // 诊断栏（排查"互相看不见"）
            Text(
                "$udp · 收包 $rx",
                fontSize = 10.sp,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 3.dp),
            )
        }
    }

    if (showAdd) AddDeviceDialog(
        onScan = { activity?.startScan(); showAdd = false },
        onManual = { App.addManual(it); showAdd = false },
        onDismiss = { showAdd = false },
    )
    if (showQr) MyQrDialog(onDismiss = { showQr = false })
    if (showRename) RenameDialog(onDismiss = { showRename = false })
}

/** 改名：随机重掷 + 手动输入 */
@Composable
fun RenameDialog(onDismiss: () -> Unit) {
    var name by remember { mutableStateOf(App.me.name) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("设备名", fontWeight = FontWeight.SemiBold) },
        text = {
            Column {
                OutlinedTextField(name, { name = it },
                    modifier = Modifier.fillMaxWidth(),
                    singleLine = true)
                Spacer(Modifier.height(8.dp))
                OutlinedButton(
                    onClick = { name = randomPoeticName() },
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Icon(Icons.Rounded.Casino, null, Modifier.size(16.dp))
                    Spacer(Modifier.width(6.dp))
                    Text("换个诗意的名字", fontSize = 13.sp)
                }
            }
        },
        confirmButton = {
            TextButton(onClick = {
                App.renameDevice(name)
                onDismiss()
            }) { Text("保存") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("取消") } },
    )
}

@Composable
fun DeviceRow(p: Peer, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // 首字头像
        Box(
            Modifier.size(42.dp).clip(CircleShape)
                .background(Indigo.copy(alpha = 0.18f)),
            contentAlignment = Alignment.Center,
        ) {
            Text(p.info.name.take(1), color = Indigo,
                fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(p.info.name, fontWeight = FontWeight.Medium)
            Text("${p.info.plat} · ${p.addr.hostAddress}:${p.info.port}",
                fontSize = 12.sp,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        // 在线点 + 手动标记
        if (p.manual) {
            Text("手动", fontSize = 10.sp, color = MaterialTheme.colorScheme
                .onSurfaceVariant,
                modifier = Modifier.border(0.5.dp, MaterialTheme.colorScheme
                    .onSurfaceVariant.copy(alpha = 0.4f), RoundedCornerShape(4.dp))
                    .padding(horizontal = 5.dp, vertical = 1.dp))
            Spacer(Modifier.width(8.dp))
        }
        Box(Modifier.size(8.dp).clip(CircleShape)
            .background(androidx.compose.ui.graphics.Color(0xFF22C55E)))
    }
}

/** 添加设备：扫码 / 手动输入 */
@Composable
fun AddDeviceDialog(onScan: () -> Unit, onManual: (String) -> Unit, onDismiss: () -> Unit) {
    var addr by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("添加设备", fontWeight = FontWeight.SemiBold) },
        text = {
            Column {
                Text("方式一：电脑端点侧栏二维码，手机扫码", fontSize = 13.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                Spacer(Modifier.height(8.dp))
                Button(onClick = onScan, modifier = Modifier.fillMaxWidth()) {
                    Text("扫码添加")
                }
                Spacer(Modifier.height(16.dp))
                Text("方式二：手动输入地址", fontSize = 13.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                Spacer(Modifier.height(8.dp))
                OutlinedTextField(addr, { addr = it },
                    modifier = Modifier.fillMaxWidth(),
                    placeholder = { Text("192.168.1.5 或 192.168.1.5:17878") },
                    singleLine = true)
            }
        },
        confirmButton = {
            TextButton(onClick = { if (addr.isNotBlank()) onManual(addr) }) {
                Text("连接")
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("取消") } },
    )
}

/** 本机二维码（其他设备扫码添加本机） */
@Composable
fun MyQrDialog(onDismiss: () -> Unit) {
    val ip = remember { Discovery.myLanIp() ?: "127.0.0.1" }
    val content = "http://$ip:${App.me.port}"
    val bmp = remember(content) { MainActivity.qrBitmap(content) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("本机二维码", fontWeight = FontWeight.SemiBold) },
        text = {
            Column(horizontalAlignment = Alignment.CenterHorizontally,
                modifier = Modifier.fillMaxWidth()) {
                if (bmp != null) {
                    Image(bmp.asImageBitmap(), null,
                        Modifier.size(220.dp).background(androidx.compose.ui.graphics.Color.White)
                            .padding(10.dp).clip(RoundedCornerShape(12.dp)))
                }
                Spacer(Modifier.height(10.dp))
                Text(content, fontSize = 13.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("关闭") } },
    )
}

// ---------------------------------------------------------------- 会话

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
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.size(34.dp).clip(CircleShape)
                            .background(Indigo.copy(alpha = 0.18f)),
                            contentAlignment = Alignment.Center) {
                            Text((peer?.info?.name ?: "?").take(1),
                                color = Indigo, fontSize = 14.sp,
                                fontWeight = FontWeight.SemiBold)
                        }
                        Spacer(Modifier.width(10.dp))
                        Column {
                            Text(peer?.info?.name ?: "设备",
                                fontWeight = FontWeight.SemiBold, fontSize = 16.sp)
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Box(Modifier.size(6.dp).clip(CircleShape)
                                    .background(if (peer != null)
                                        androidx.compose.ui.graphics.Color(0xFF22C55E)
                                    else MaterialTheme.colorScheme.onSurfaceVariant))
                                Spacer(Modifier.width(4.dp))
                                Text(if (peer != null) "在线" else "离线",
                                    fontSize = 11.sp,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    }
                },
                navigationIcon = {
                    TextButton(onClick = { App.currentPeer = null }) {
                        Text("←", fontSize = 18.sp)
                    }
                },
            )
        },
    ) { pad ->
        Column(Modifier.padding(pad).fillMaxSize().imePadding()) {
            App.sendStatus?.let {
                Text(it, fontSize = 12.sp, color = Indigo,
                    modifier = Modifier.padding(horizontal = 14.dp, vertical = 4.dp))
            }
            LazyColumn(Modifier.weight(1f).padding(horizontal = 10.dp),
                reverseLayout = false) {
                items(msgs.size + progresses.size) { i ->
                    if (i < msgs.size) MessageBubble(msgs[i])
                    else ProgressCard(progresses[i - msgs.size])
                }
            }
            // 紧凑输入条：小图标按钮 + 胶囊输入框 + 圆形发送
            Row(Modifier.padding(horizontal = 10.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = { activity?.pick() },
                    modifier = Modifier.size(36.dp)) {
                    Icon(Icons.Rounded.AttachFile, "选择文件",
                        modifier = Modifier.size(18.dp),
                        tint = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Spacer(Modifier.width(4.dp))
                OutlinedTextField(
                    value = input,
                    onValueChange = { v -> input = v },
                    modifier = Modifier.weight(1f),
                    placeholder = { Text("消息", fontSize = 13.sp) },
                    maxLines = 3,
                    shape = RoundedCornerShape(18.dp),
                )
                Spacer(Modifier.width(4.dp))
                if (input.isNotBlank()) {
                    SmallFloatingActionButton(onClick = {
                        App.sendText(peerId, input); input = ""
                    }, shape = CircleShape,
                        containerColor = IndigoDark,
                        contentColor = androidx.compose.ui.graphics.Color.White,
                        modifier = Modifier.size(36.dp)) {
                        Icon(Icons.AutoMirrored.Rounded.Send, "发送",
                            modifier = Modifier.size(16.dp))
                    }
                }
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
            shape = RoundedCornerShape(
                topStart = 16.dp, topEnd = 16.dp,
                bottomStart = if (end) 16.dp else 4.dp,
                bottomEnd = if (end) 4.dp else 16.dp),
            color = if (end) IndigoDark
            else MaterialTheme.colorScheme.surfaceVariant,
        ) {
            Column(Modifier.padding(12.dp).widthIn(max = 280.dp)) {
                when (e) {
                    is ChatEntry.Text -> Text(e.text,
                        color = if (end) androidx.compose.ui.graphics.Color.White
                        else MaterialTheme.colorScheme.onSurface)
                    is ChatEntry.FileCard -> Column(
                        Modifier.clickable {
                            e.files.firstOrNull()?.let {
                                MainActivity.openFile(ctx, it)
                            }
                        }) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(if (e.files.size > 1 || e.files.isEmpty()) "📁" else "📄",
                                fontSize = 18.sp)
                            Spacer(Modifier.width(8.dp))
                            Text(e.title, maxLines = 1, overflow = TextOverflow.Ellipsis,
                                fontWeight = FontWeight.Medium,
                                color = if (end) androidx.compose.ui.graphics.Color.White
                                else MaterialTheme.colorScheme.onSurface)
                        }
                        e.location?.let {
                            Text(it, fontSize = 10.sp, color =
                                if (end) androidx.compose.ui.graphics.Color.White.copy(alpha = 0.7f)
                                else MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        Text("${fmtSize(e.size)} · 点击打开", fontSize = 11.sp,
                            color = if (end)
                                androidx.compose.ui.graphics.Color.White.copy(alpha = 0.8f)
                            else MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    is ChatEntry.SendProgress -> {}
                }
            }
        }
    }
}

@Composable
fun ProgressCard(p: RecvProgress) {
    Surface(
        shape = RoundedCornerShape(16.dp, 16.dp, 16.dp, 4.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
    ) {
        Column(Modifier.padding(12.dp).widthIn(max = 280.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("📁", fontSize = 18.sp)
                Spacer(Modifier.width(8.dp))
                Text(p.label, fontWeight = FontWeight.Medium,
                    maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text("  ${p.fileIdx + 1}/${p.fileCount}", fontSize = 11.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Spacer(Modifier.height(6.dp))
            val frac = if (p.total > 0) p.transferred.toFloat() / p.total else 0f
            Text("接收中 ${(frac * 100).toInt()}% · " +
                    "${fmtSize(p.transferred)} / ${fmtSize(p.total)}",
                fontSize = 11.sp,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(6.dp))
            LinearProgressIndicator(
                progress = { frac },
                modifier = Modifier.fillMaxWidth().height(5.dp)
                    .clip(RoundedCornerShape(3.dp)),
                color = Indigo,
            )
        }
    }
}
