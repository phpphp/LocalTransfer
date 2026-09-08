package io.github.localtransfer

import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
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
import androidx.compose.foundation.combinedClickable
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
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.Computer
import androidx.compose.material.icons.rounded.Edit
import androidx.compose.material.icons.rounded.Language
import androidx.compose.material.icons.rounded.DesktopWindows
import androidx.compose.material.icons.rounded.Devices
import androidx.compose.material.icons.rounded.Folder
import androidx.compose.material.icons.rounded.LaptopMac
import androidx.compose.material.icons.rounded.PhoneAndroid
import androidx.compose.material.icons.rounded.PhoneIphone
import androidx.compose.material.icons.rounded.QrCode2
import androidx.compose.material.icons.rounded.Settings
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

    // 通知权限（前台服务通知，API 33+）
    private val notifPerm =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { }

    // 接收目录选择器（必须在 Activity 初始化期注册，Composable 内注册会崩）
    val pickDirLauncher =
        registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
            uri ?: return@registerForActivityResult
            val ctx = applicationContext
            val docId = try {
                android.provider.DocumentsContract.getTreeDocumentId(uri)
            } catch (_: Exception) { null }
            val path = docId?.let { id ->
                when {
                    id.startsWith("primary:") ->
                        "/storage/emulated/0/" + id.removePrefix("primary:")
                    id.startsWith("/") -> id
                    else -> null
                }
            }
            if (path != null) {
                ctx.getSharedPreferences("lt", Context.MODE_PRIVATE)
                    .edit().putString("save_dir", path).apply()
                App.server.customSaveDir = path
                App.saveDirDisplay = path
                val writable = File(path).canWrite()
                MainActivity.toast(ctx,
                    if (writable) "已设为 $path"
                    else "已设为 $path（当前不可写，请检查权限）")
            } else {
                MainActivity.toast(ctx, "无法识别该目录的真实路径，请换一个")
            }
        }

    fun pickDir() = pickDirLauncher.launch(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        App.init(applicationContext)
        setContent { App() }
        // 通知权限（API 33+，前台服务通知用）
        if (Build.VERSION.SDK_INT >= 33) {
            notifPerm.launch(android.Manifest.permission.POST_NOTIFICATIONS)
        }
        // 前台服务：后台持续可接收
        startForegroundService(Intent(this, TransferService::class.java))
    }

    fun startScan() {
        // 自定义竖屏卡片式扫码页（库自带 CaptureActivity 是横屏满屏布局）
        scanLauncher.launch(ScanOptions().apply {
            setPrompt("对准电脑端二维码（http://IP:端口）")
            setBeepEnabled(false)
            setDesiredBarcodeFormats(ScanOptions.QR_CODE)
            setCaptureActivity(PortraitCaptureActivity::class.java)
        })
    }

    fun pick() = pickFiles.launch("*/*")

    companion object {
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
    /** 需要"所有文件访问"权限（首次启动 / 权限被收回）→ UI 弹窗引导 */
    var needsAllFilesPermission by mutableStateOf(false)
    /** 当前接收目录（弹窗显示用，选择后立即更新） */
    var saveDirDisplay by mutableStateOf("")
    /** 自动接收文件 */
    var autoReceive by mutableStateOf(false)
    /** 主题模式：null=跟随系统 */
    var themeMode by mutableStateOf<String?>(null)
    /** 本机设备名（系统"设备名称"，回落型号；改名弹窗"使用本机设备名"用） */
    var phoneModel by mutableStateOf("")
    /** 速度采样表：token → (纳秒时间, 累计字节, 上次速度) */
    internal val speedSamples = HashMap<String, Triple<Long, Long, Double>>()

    /** 传输速度采样（收发共用）：<400ms 沿用旧值；字节回退=换文件，重置基线保留旧速度。
     *  只在主线程调（收发回调都经 main.post 到这里）。 */
    fun sampleSpeed(key: String, transferred: Long): Double {
        val now = System.nanoTime()
        val prev = speedSamples[key]
        if (prev != null) {
            val dt = (now - prev.first) / 1e9
            if (dt < 0.4) return prev.third
            val delta = transferred - prev.second
            val speed = if (delta > 0) delta / dt else prev.third
            speedSamples[key] = Triple(now, transferred, speed)
            return speed
        }
        speedSamples[key] = Triple(now, transferred, 0.0)
        return 0.0
    }

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
        // 本机设备名：系统设置里的"设备名称"（用户自定义过的，API 25+）
        // → 型号 Build.MODEL → 空字符串（键名用字面量避开 API 24 的常量 lint）
        phoneModel = android.provider.Settings.Global
            .getString(ctx.contentResolver, "device_name")
            ?.trim()?.ifBlank { null }
            ?: Build.MODEL.trim().ifBlank { null }
            ?: ""
        // 默认设备名 = 本机设备名；未存过时用它，不落库
        //（用户改过名 / 点过"随机"才持久化，保持系统名的动态性）
        themeMode = prefs.getString("theme_mode", null)
        val name = prefs.getString("device_name", null)
            ?: phoneModel.ifBlank { randomPoeticName().also {
                prefs.edit().putString("device_name", it).apply() } }
        autoReceive = prefs.getBoolean("auto_receive", false)
        me = DeviceInfo(id, name, "android", 0)
        api = TransferApi(me)
        server = MiniHttpServer(me, ctx, object : MiniHttpServer.Callbacks {
            override fun onMessage(peerId: String, peerName: String, text: String) {
                chats.getOrPut(peerId) { mutableStateListOf<ChatEntry>() }
                    .add(ChatEntry.Text(false, text, System.currentTimeMillis()))
            }
            override fun onIncoming(req: IncomingReq) {
                if (autoReceive) {
                    req.decision.complete(true)   // 静默接收，不弹窗不 Toast
                } else {
                    pendingReq = req
                }
            }
            override fun onProgress(token: String, p: RecvProgress) {
                progress[token] = p.copy(speedBps = sampleSpeed(token, p.transferred))
            }
            override fun onBatchDone(peerId: String, files: List<ReceivedFile>) {
                progress.keys.filter { !it.startsWith("send") }
                    .forEach { progress.remove(it); speedSamples.remove(it) }
                // 位置 = 实际写入的目录（server.customSaveDir），失败时显示原因
                val location = server.customSaveDir
                    ?: files.firstNotNullOfOrNull { it.path }?.let {
                        it.substringBeforeLast('/')
                    } ?: "Download/LocalTransfer"
                val finalLocation = server.publishError ?: location
                chats.getOrPut(peerId) { mutableStateListOf<ChatEntry>() }.add(ChatEntry.FileCard(
                    false,
                    if (files.size == 1) files[0].name else "${files.size} 个文件",
                    files.sumOf { it.size },
                    System.currentTimeMillis(),
                    finalLocation, files))
            }
        })
        me = me.copy(port = server.start(DEFAULT_HTTP_PORT))
        server.setIdentity(me)   // /api/info 返回带真实端口的身份
        // 默认接收目录：Download/LocalTransfer（直接 File 写入）；
        // 用户自定义过的（save_dir）优先。首次启动没权限时由 UI 弹窗引导授权。
        val saved = prefs.getString("save_dir", "")?.ifBlank { null }
        server.customSaveDir = saved ?: "/storage/emulated/0/Download/LocalTransfer"
        saveDirDisplay = server.customSaveDir ?: ""
        needsAllFilesPermission = Build.VERSION.SDK_INT >= 30 &&
                !Environment.isExternalStorageManager()
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
        chats.getOrPut(peerId) { mutableStateListOf<ChatEntry>() }
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
            val sendKey = "send-${System.currentTimeMillis()}"
            val totalLabel = run {
                // 先读文件名+大小（拷贝到缓存，content URI 无法直接二次流式读）
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
                metas
            }
            if (totalLabel.isEmpty()) { sendStatus = null; return@launch }
            val metas = totalLabel
            val title = if (metas.size == 1) metas[0].first.name
                        else "${metas.size} 个文件"
            val sum = metas.sumOf { it.first.size }
            // 发送进度卡（消息流里，替代顶部状态条）——
            // 从"等待对方接收"起就带 sending=true（右侧），不再先左后右跳
            main.post {
                progress[sendKey] = RecvProgress(peerId, title, 0, metas.size, 0, sum,
                    sending = true, waiting = true)
            }
            runCatching {
                api.sendFiles(p, metas) { i, t, tot ->
                    main.post {
                        progress[sendKey] = RecvProgress(peerId, title, i, metas.size,
                            t, tot, sampleSpeed(sendKey, t), sending = true)
                    }
                }
                main.post {
                    progress.remove(sendKey)
                    speedSamples.remove(sendKey)
                    // 发送完成的卡片带源文件路径
                    val sent = metas.map { m ->
                        ReceivedFile(m.first.name, m.first.size, null, m.second)
                    }
                    chats.getOrPut(peerId) { mutableStateListOf<ChatEntry>() }.add(ChatEntry.FileCard(
                        true, title, sum, System.currentTimeMillis(), null, sent))
                }
            }.onFailure {
                main.post {
                    progress.remove(sendKey)
                    speedSamples.remove(sendKey)
                    sendStatus = it.message
                }
            }
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
    val ctx = LocalContext.current
    BackInterceptor(ctx)
    // 主题：跟随系统 / 强制亮色 / 强制暗色
    val isDark = when (App.themeMode) {
        "light" -> false
        "dark" -> true
        else -> androidx.compose.foundation.isSystemInDarkTheme()
    }
    val colors = if (isDark) darkColorScheme(primary = Indigo)
                 else lightColorScheme(primary = IndigoDark)
    MaterialTheme(colorScheme = colors) {
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
                    // 点接收 → 直接跳进对应会话（看进度），不用再手动找设备
                    App.currentPeer = req.peer.id
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
    val online = peers.values.filter {
        it.manual || System.currentTimeMillis() - it.lastSeen < DEVICE_TIMEOUT_MS
    }.sortedBy { it.info.name }

    var showAdd by remember { mutableStateOf(false) }
    var showQr by remember { mutableStateOf(false) }
    var showRename by remember { mutableStateOf(false) }
    var showSettings by remember { mutableStateOf(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    // 上面 App 图标+名称；下面 头像+名字+改名图标
                    Row(verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier.fillMaxWidth()) {
                        // App 图标
                        Box(Modifier.size(34.dp).clip(CircleShape)
                            .background(Indigo.copy(alpha = 0.18f)),
                            contentAlignment = Alignment.Center) {
                            Icon(Icons.Rounded.Language, "LocalTransfer",
                                tint = Indigo, modifier = Modifier.size(18.dp))
                        }
                        Spacer(Modifier.width(8.dp))
                        Column {
                            Text("LocalTransfer", fontSize = 11.sp,
                                color = MaterialTheme.colorScheme.onSurfaceVariant)
                            // 名字 + 改名按钮
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(App.me.name, fontWeight = FontWeight.SemiBold,
                                    fontSize = 15.sp)
                                Spacer(Modifier.width(2.dp))
                                Icon(Icons.Rounded.Edit, "改名",
                                    modifier = Modifier
                                        .size(13.dp)
                                        .clickable { showRename = true },
                                    tint = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    }
                },
                actions = {
                    // 设置（接收目录等）
                    IconButton(onClick = { showSettings = true }) {
                        Icon(Icons.Rounded.Settings, "设置")
                    }
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
        }
    }

    if (showAdd) AddDeviceDialog(
        onScan = { activity?.startScan(); showAdd = false },
        onManual = { App.addManual(it); showAdd = false },
        onDismiss = { showAdd = false },
    )
    if (showQr) MyQrDialog(onDismiss = { showQr = false })
    if (showRename) RenameDialog(onDismiss = { showRename = false })
    if (showSettings) SettingsDialog(onDismiss = { showSettings = false })
    // 权限引导：需要"所有文件访问"时弹窗（首次启动 / 权限被收回 / 接收时写失败）
    LaunchedEffect(Unit) {
        while (true) {
            if (App.needsAllFilesPermission || App.server.permissionNeeded) {
                App.server.permissionNeeded = false
                App.needsAllFilesPermission = true
            }
            kotlinx.coroutines.delay(1000)
        }
    }
    if (App.needsAllFilesPermission &&
        Build.VERSION.SDK_INT >= 30 && !Environment.isExternalStorageManager()) {
        AllFilesPermissionDialog()
    }
}

/** 返回手势：会话页返回列表；列表页双击退出（全面屏手势不再直接最小化） */
@Composable
fun BackInterceptor(ctx: Context) {
    var lastBack by remember { mutableStateOf(0L) }
    androidx.activity.compose.BackHandler(enabled = true) {
        if (App.currentPeer != null) {
            App.currentPeer = null
        } else {
            val now = System.currentTimeMillis()
            if (now - lastBack < 2000) {
                (ctx as? ComponentActivity)?.finish()
            } else {
                lastBack = now
                MainActivity.toast(ctx, "再按一次退出")
            }
        }
    }
}

/** 权限引导弹窗（可跳过，收文件时会再弹） */
@Composable
fun AllFilesPermissionDialog() {
    val ctx = LocalContext.current
    AlertDialog(
        onDismissRequest = { App.needsAllFilesPermission = false },
        title = { Text("需要存储权限", fontWeight = FontWeight.SemiBold) },
        text = {
            Text("接收的文件将保存到 Download/LocalTransfer。\n" +
                    "Android 11+ 写入公共目录需要「所有文件访问」权限，" +
                    "请在下一页中开启。")
        },
        confirmButton = {
            TextButton(onClick = {
                App.needsAllFilesPermission = false
                val i = Intent(
                    android.provider.Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                    Uri.parse("package:${ctx.packageName}"))
                runCatching { ctx.startActivity(i) }
            }) { Text("去授权") }
        },
        dismissButton = {
            TextButton(onClick = { App.needsAllFilesPermission = false }) {
                Text("暂不")
            }
        },
    )
}

/** 设置：接收目录（系统文件夹选择器选目录，不手输路径；
 *  默认 Download/LocalTransfer，"恢复默认"一键回设） */
@Composable
fun SettingsDialog(onDismiss: () -> Unit) {
    val ctx = LocalContext.current
    val prefs = remember { ctx.getSharedPreferences("lt", Context.MODE_PRIVATE) }
    val activity = ctx as? MainActivity
    // 当前生效目录（选择器回调在 Activity 侧写 prefs，弹窗重建时读最新）
    // 直接读响应式状态——目录选择后弹窗立即刷新
    val currentDir = App.saveDirDisplay.ifBlank { "/storage/emulated/0/Download/LocalTransfer" }
    val hasAllFiles = Build.VERSION.SDK_INT < 30 || Environment.isExternalStorageManager()

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("设置", fontWeight = FontWeight.SemiBold) },
        text = {
            Column {
                Text("接收目录", fontSize = 13.sp)
                Spacer(Modifier.height(6.dp))
                Text(currentDir, fontSize = 12.sp,
                    color = MaterialTheme.colorScheme.primary)
                Spacer(Modifier.height(8.dp))
                Button(onClick = { activity?.pickDir() },
                    modifier = Modifier.fillMaxWidth()) {
                    Icon(Icons.Rounded.Folder, null, Modifier.size(16.dp))
                    Spacer(Modifier.width(6.dp))
                    Text("选择文件夹…", fontSize = 13.sp)
                }
                Spacer(Modifier.height(6.dp))
                OutlinedButton(
                    onClick = {
                        val def = "/storage/emulated/0/Download/LocalTransfer"
                        prefs.edit().putString("save_dir", def).apply()
                        App.server.customSaveDir = def
                        App.saveDirDisplay = def
                    },
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text("恢复默认（Download/LocalTransfer）", fontSize = 12.sp)
                }
                Spacer(Modifier.height(16.dp))
                // 自动接收
                Row(verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.fillMaxWidth()) {
                    Column(Modifier.weight(1f)) {
                        Text("自动接收文件", fontSize = 13.sp)
                        Text("跳过确认直接保存", fontSize = 11.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    Switch(checked = App.autoReceive, onCheckedChange = { on ->
                        App.autoReceive = on
                        prefs.edit().putBoolean("auto_receive", on).apply()
                    })
                }
                Spacer(Modifier.height(16.dp))
                // 外观主题（null=跟随系统；putString(null) 即清除，回落跟随系统）
                Row(Modifier.fillMaxWidth()) {
                    listOf(null to "跟随系统", "light" to "浅色", "dark" to "深色")
                        .forEachIndexed { i, (mode, label) ->
                            if (i > 0) Spacer(Modifier.width(6.dp))
                            FilterChip(
                                selected = App.themeMode == mode,
                                onClick = {
                                    App.themeMode = mode
                                    prefs.edit().putString("theme_mode", mode).apply()
                                },
                                label = { Text(label, fontSize = 12.sp) },
                                modifier = Modifier.weight(1f),
                            )
                        }
                }
                if (!hasAllFiles) {
                    Spacer(Modifier.height(8.dp))
                    Text("⚠ 未授予「所有文件访问」权限，写入公共目录会失败。" +
                            "接收文件时会弹窗提醒。",
                        fontSize = 11.sp,
                        color = androidx.compose.ui.graphics.Color(0xFFEF4444))
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text("关闭") }
        },
    )
}

/** 改名：手动输入 + 本机设备名 + 随机诗意名 */
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
                // 使用本机型号
                if (App.phoneModel.isNotBlank()) {
                    OutlinedButton(
                        onClick = { name = App.phoneModel },
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Icon(Icons.Rounded.PhoneAndroid, null, Modifier.size(16.dp))
                        Spacer(Modifier.width(6.dp))
                        Text("使用本机设备名（${App.phoneModel}）", fontSize = 12.sp)
                    }
                    Spacer(Modifier.height(6.dp))
                }
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

/** 平台 → 设备类型图标（Material 真实图标） */
@Composable
fun platIcon(plat: String): androidx.compose.ui.graphics.vector.ImageVector =
    when (plat.lowercase()) {
        "windows" -> Icons.Rounded.DesktopWindows
        "mac", "macos" -> Icons.Rounded.LaptopMac
        "linux" -> Icons.Rounded.Computer
        "ios" -> Icons.Rounded.PhoneIphone
        "android" -> Icons.Rounded.PhoneAndroid
        else -> Icons.Rounded.Devices
    }

/** 平台显示名 */
fun platLabel(plat: String): String = when (plat.lowercase()) {
    "windows" -> "Windows"
    "mac", "macos" -> "Mac"
    "linux" -> "Linux"
    "ios" -> "iPhone"
    "android" -> "Android"
    "web" -> "网页"
    else -> plat
}

@Composable
fun DeviceRow(p: Peer, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // 首字头像（设备名首字）
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
            // 副标题：设备类型图标 + 平台名 · 地址
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(platIcon(p.info.plat), platLabel(p.info.plat),
                    modifier = Modifier.size(13.dp),
                    tint = MaterialTheme.colorScheme.onSurfaceVariant)
                Spacer(Modifier.width(3.dp))
                Text("${platLabel(p.info.plat)} · ${p.addr.hostAddress}:${p.info.port}",
                    fontSize = 12.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
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

/** 添加设备：扫码 / 手动输入（IP + 端口分框） */
@Composable
fun AddDeviceDialog(onScan: () -> Unit, onManual: (String) -> Unit, onDismiss: () -> Unit) {
    var ip by remember { mutableStateOf("") }
    var port by remember { mutableStateOf("17878") }
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
                Text("方式二：手动输入", fontSize = 13.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                Spacer(Modifier.height(8.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(ip, { ip = it },
                        modifier = Modifier.weight(1f),
                        placeholder = { Text("IP 地址", fontSize = 13.sp) },
                        singleLine = true,
                        textStyle = androidx.compose.ui.text.TextStyle(fontSize = 14.sp))
                    Spacer(Modifier.width(8.dp))
                    OutlinedTextField(port, { port = it },
                        modifier = Modifier.width(96.dp),
                        placeholder = { Text("端口", fontSize = 13.sp) },
                        singleLine = true,
                        textStyle = androidx.compose.ui.text.TextStyle(fontSize = 14.sp))
                }
            }
        },
        confirmButton = {
            TextButton(onClick = {
                if (ip.isNotBlank()) {
                    val p = port.trim().toIntOrNull() ?: 17878
                    onManual(if (p == 17878) ip else "$ip:$p")
                }
            }) {
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
    val msgs = remember(peerId) { App.chats.getOrPut(peerId) { mutableStateListOf<ChatEntry>() } }
    var input by remember { mutableStateOf("") }
    val progresses = App.progress.values.filter { it.peerId == peerId }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        // 首字头像（设备名首字）
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
                            // 副标题：在线点 + 设备类型图标 + "平台名 · 在线"
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Box(Modifier.size(6.dp).clip(CircleShape)
                                    .background(if (peer != null)
                                        androidx.compose.ui.graphics.Color(0xFF22C55E)
                                    else MaterialTheme.colorScheme.onSurfaceVariant))
                                Spacer(Modifier.width(4.dp))
                                Icon(platIcon(peer?.info?.plat ?: ""),
                                    platLabel(peer?.info?.plat ?: ""),
                                    modifier = Modifier.size(12.dp),
                                    tint = MaterialTheme.colorScheme.onSurfaceVariant)
                                Spacer(Modifier.width(3.dp))
                                Text(
                                    if (peer != null)
                                        "${platLabel(peer.info.plat)} · 在线"
                                    else "离线",
                                    fontSize = 11.sp,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    }
                },
                navigationIcon = {
                    IconButton(onClick = { App.currentPeer = null }) {
                        Icon(Icons.AutoMirrored.Rounded.ArrowBack, "返回")
                    }
                },
            )
        },
    ) { pad ->
        Column(Modifier.padding(pad).fillMaxSize().imePadding()) {
            // 错误提示（进度走消息流卡片，不占标题区）
            App.sendStatus?.let {
                Text(it, fontSize = 11.sp,
                    color = androidx.compose.ui.graphics.Color(0xFFEF4444),
                    modifier = Modifier.padding(horizontal = 14.dp, vertical = 2.dp))
            }
            LazyColumn(Modifier.weight(1f).padding(horizontal = 10.dp),
                reverseLayout = false) {
                items(msgs.size + progresses.size) { i ->
                    if (i < msgs.size) MessageBubble(msgs[i],
                        onDelete = { en ->
                            // 引用相等删这一条（值相等会误删同内容的重复消息）
                            val idx = msgs.indexOfFirst { it === en }
                            if (idx >= 0) msgs.removeAt(idx)
                        })
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

/** 消息手势：双击=直接复制（文本），长按=弹菜单（文本=复制/删除，文件卡=删除） */
@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
fun MessageBubble(e: ChatEntry, onDelete: (ChatEntry) -> Unit) {
    val ctx = LocalContext.current
    val clip = androidx.compose.ui.platform.LocalClipboardManager.current
    val end = e.outgoing
    var menu by remember { mutableStateOf(false) }
    var lastTap by remember { mutableStateOf(0L) }
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
            Column(
                Modifier.padding(12.dp).widthIn(max = 280.dp)
                    .combinedClickable(
                        interactionSource = remember {
                            androidx.compose.foundation.interaction.MutableInteractionSource() },
                        indication = null,
                        onClick = {
                            // 双击（350ms 内两次点击）= 直接复制文本
                            val now = System.currentTimeMillis()
                            if (e is ChatEntry.Text && now - lastTap < 350) {
                                clip.setText(
                                    androidx.compose.ui.text.AnnotatedString(e.text))
                                MainActivity.toast(ctx, "已复制")
                            }
                            lastTap = now
                        },
                        onLongClick = { menu = true },
                    )) {
                when (e) {
                    is ChatEntry.Text -> Text(e.text,
                        color = if (end) androidx.compose.ui.graphics.Color.White
                        else MaterialTheme.colorScheme.onSurface)
                    // 文件卡只展示，不可点击（用户明确要求）
                    is ChatEntry.FileCard -> Column {
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
                        Text(fmtSize(e.size), fontSize = 11.sp,
                            color = if (end)
                                androidx.compose.ui.graphics.Color.White.copy(alpha = 0.8f)
                            else MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    is ChatEntry.SendProgress -> {}
                }
            }
        }
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            if (e is ChatEntry.Text) {
                DropdownMenuItem(
                    text = { Text("复制") },
                    onClick = {
                        clip.setText(
                            androidx.compose.ui.text.AnnotatedString(e.text))
                        MainActivity.toast(ctx, "已复制")
                        menu = false
                    })
            }
            DropdownMenuItem(
                text = { Text("删除") },
                onClick = { menu = false; onDelete(e) })
        }
    }
}

@Composable
fun ProgressCard(p: RecvProgress) {
    // 发送靠右、接收靠左（与完成后的文件卡同侧，不再"传完跳边"）
    Box(Modifier.fillMaxWidth().padding(vertical = 4.dp),
        contentAlignment = if (p.sending) Alignment.CenterEnd else Alignment.CenterStart) {
        Surface(
            // 尖角朝发送方：右下（发送）/ 左下（接收）
            shape = if (p.sending) RoundedCornerShape(16.dp, 16.dp, 4.dp, 16.dp)
                    else RoundedCornerShape(16.dp, 16.dp, 16.dp, 4.dp),
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
            // 等待确认 → 只显示等待文案；速度 >0 才显示
            val status = if (p.waiting) "等待对方接收…"
                else "${if (p.sending) "发送中" else "接收中"} ${(frac * 100).toInt()}%" +
                    (if (p.speedBps > 0) " · ${fmtSize(p.speedBps.toLong())}/s" else "")
            Text(status,
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
}
