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
import androidx.compose.material.icons.automirrored.rounded.DriveFileMove
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Description
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.FileDownload
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.Computer
import androidx.compose.material.icons.rounded.ContentCopy
import androidx.compose.material.icons.rounded.ContentPaste
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material.icons.rounded.DeleteSweep
import androidx.compose.material.icons.rounded.Edit
import androidx.compose.material.icons.rounded.Language
import androidx.compose.material.icons.rounded.DesktopWindows
import androidx.compose.material.icons.rounded.Devices
import androidx.compose.material.icons.rounded.Folder
import androidx.compose.material.icons.rounded.Image
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.InsertDriveFile
import androidx.compose.material.icons.rounded.Chat
import androidx.compose.material.icons.rounded.KeyboardArrowDown
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

    // 系统相册选择（Photo Picker：照片/截图/视频多选，免存储权限）
    private val pickMedia =
        registerForActivityResult(ActivityResultContracts.PickMultipleVisualMedia()) { uris ->
            if (uris.isNotEmpty()) App.sendPicked(uris)
        }

    // 应用内相册（微信式网格）权限：13+ 细分媒体权限，旧版外存权限
    private val galleryPerm =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { grants ->
            if (grants.values.any { it }) {
                App.showGallery = true
            } else {
                MainActivity.toast(this, "需要相册权限才能选择照片/视频")
            }
        }

    // 发送文件夹：SAF 选树 → DocumentFile 递归收集（uri → 带目录前缀的相对路径）
    private val pickFolder =
        registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
            uri?.let { App.sendFolder(it) }
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
                // 接收弹窗的"存到…"：只临时改本次接收目录（不落库、不动设置显示），
                // 批次完成自动恢复默认
                if (App.pickingForReceive) {
                    App.pickingForReceive = false
                    App.server.customSaveDir = path
                    // 完成队头那个请求（正在弹窗展示的）
                    App.pendingReqs.firstOrNull()?.let { r ->
                        r.decision.complete(true)
                        App.currentPeer = r.peer.id
                        App.pendingReqs.remove(r)
                        MainActivity.toast(ctx, "本批将保存到 $path")
                    }
                    return@registerForActivityResult
                }
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
        // 通知权限（API 33+，前台服务通知与消息提醒用）
        if (Build.VERSION.SDK_INT >= 33) {
            notifPerm.launch(android.Manifest.permission.POST_NOTIFICATIONS)
        }
        // 前台服务：后台持续可接收
        startForegroundService(Intent(this, TransferService::class.java))
    }

    override fun onResume() { super.onResume(); App.isForeground = true }
    override fun onPause() { super.onPause(); App.isForeground = false }

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
    fun pickPhotos() {
        val perms = mutableListOf<String>()
        if (Build.VERSION.SDK_INT >= 33) {
            perms += listOf(android.Manifest.permission.READ_MEDIA_IMAGES,
                android.Manifest.permission.READ_MEDIA_VIDEO)
        } else {
            perms += android.Manifest.permission.READ_EXTERNAL_STORAGE
        }
        galleryPerm.launch(perms.toTypedArray())
    }
    fun pickFolder() = pickFolder.launch(null)

    companion object {
        fun toast(context: Context, text: String) {
            android.widget.Toast.makeText(context, text,
                android.widget.Toast.LENGTH_SHORT).show()
        }

        /** 打开文件。URI 用 content://（MediaStore，可跨应用授权）；
         *  真实路径用 FileProvider（file:// 直传 API 24+ 抛 FileUriExposedException） */
        fun openFile(context: Context, f: ReceivedFile) {
            try {
                val intent = Intent(Intent.ACTION_VIEW).apply {
                    val uri = f.uri?.let { Uri.parse(it) }
                        ?: f.path?.let {
                            androidx.core.content.FileProvider.getUriForFile(
                                context, "${context.packageName}.fileprovider", File(it))
                        } ?: run {
                            toast(context, "文件路径不可用"); return
                        }
                    setDataAndType(uri, mimeOf(f.name))
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                }
                context.startActivity(intent)
            } catch (e: Exception) {
                toast(context, "打开失败：${e.message}")
            }
        }

        /** 扩展名 → MIME：决定系统用哪个应用打开——
         *  apk → 包安装器；图片/视频 → 图库（默认应用）；其余给准确 MIME 走默认应用 */
        private fun mimeOf(name: String): String = when (
            name.substringAfterLast('.', "").lowercase()) {
            "apk" -> "application/vnd.android.package-archive"
            "jpg", "jpeg" -> "image/jpeg"
            "png" -> "image/png"
            "gif" -> "image/gif"
            "webp" -> "image/webp"
            "bmp" -> "image/bmp"
            "heic", "heif" -> "image/heic"
            "mp4" -> "video/mp4"
            "webm" -> "video/webm"
            "mkv" -> "video/x-matroska"
            "mov" -> "video/quicktime"
            "3gp" -> "video/3gpp"
            "mp3" -> "audio/mpeg"
            "wav" -> "audio/wav"
            "flac" -> "audio/flac"
            "pdf" -> "application/pdf"
            "txt", "md", "log" -> "text/plain"
            "doc" -> "application/msword"
            "docx" -> "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            "xls" -> "application/vnd.ms-excel"
            "xlsx" -> "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            "ppt" -> "application/vnd.ms-powerpoint"
            "pptx" -> "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            else -> "application/octet-stream"
        }

        /** 打开目录（文件管理器定位到它）。externalstorage.documents 的
         *  文档 id 支持 primary: 前缀子路径，多数 ROM 的文件管理器认；
         *  打不开返回 false（调用方 toast 出路径兜底）。 */
        fun openDirectory(context: Context, dirPath: String): Boolean {
            val rel = dirPath.removePrefix("/storage/emulated/0/")
                .trimEnd('/')
            if (rel.isEmpty()) return false
            val docUri = runCatching {
                android.provider.DocumentsContract.buildDocumentUri(
                    "com.android.externalstorage.documents", "primary:$rel")
            }.getOrNull() ?: return false
            val i = Intent(Intent.ACTION_VIEW)
                .setDataAndType(docUri,
                    android.provider.DocumentsContract.Document.MIME_TYPE_DIR)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK
                        or Intent.FLAG_GRANT_READ_URI_PERMISSION)
            return runCatching { context.startActivity(i) }.isSuccess
        }

        /** 跳厂商"自启动 / 允许后台活动"管理页。
         *  各家都是私有页面（无标准 API），按厂商逐个尝试，兜底应用详情页。
         *  返回 false = 一个都没打开。 */
        fun jumpAutoStart(context: Context): Boolean {
            val m = Build.MANUFACTURER.lowercase()
            val candidates = mutableListOf<Intent>()
            when {
                m.contains("xiaomi") || m.contains("redmi") -> candidates += Intent()
                    .setComponent(android.content.ComponentName(
                        "com.miui.securitycenter",
                        "com.miui.permcenter.autostart.AutoStartManagementActivity"))
                m.contains("huawei") || m.contains("honor") -> candidates += Intent()
                    .setComponent(android.content.ComponentName(
                        "com.huawei.systemmanager",
                        "com.huawei.systemmanager.startupmgr.ui.StartupNormalAppListActivity"))
                m.contains("oppo") || m.contains("realme") || m.contains("oneplus") -> {
                    candidates += Intent().setComponent(android.content.ComponentName(
                        "com.coloros.safecenter",
                        "com.coloros.safecenter.permission.startup.StartupAppListActivity"))
                    candidates += Intent().setComponent(android.content.ComponentName(
                        "com.oppo.safe",
                        "com.oppo.safe.permission.startup.StartupAppListActivity"))
                }
                m.contains("vivo") || m.contains("iqoo") -> {
                    candidates += Intent().setComponent(android.content.ComponentName(
                        "com.vivo.permissionmanager",
                        "com.vivo.permissionmanager.activity.BgStartUpManagerActivity"))
                    candidates += Intent().setComponent(android.content.ComponentName(
                        "com.iqoo.secure",
                        "com.iqoo.secure.ui.phoneoptimize.BgStartUpManager"))
                }
                m.contains("meizu") -> candidates += Intent(
                    "com.meizu.safe.security.SHOW_APPSEC")
                    .putExtra("packageName", context.packageName)
            }
            // 兜底：应用详情页（原生/未识别厂商，用户手动找权限项）
            candidates += Intent(
                android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                Uri.parse("package:${context.packageName}"))
            for (i in candidates) {
                if (runCatching {
                        context.startActivity(i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                    }.isSuccess) return true
            }
            return false
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
    /** 应用内相册选择器（微信式网格）是否打开 */
    var showGallery by mutableStateOf(false)
    val chats = mutableStateMapOf<String, MutableList<ChatEntry>>()
    val progress = mutableStateMapOf<String, RecvProgress>()
    /** 待确认的接收请求队列（可同时挂多个：新请求不再顶掉旧的——
     *  曾经单槽 pendingReq 被新请求覆盖，旧请求的 Completer 永不完成，对端只能等超时） */
    val pendingReqs = mutableStateListOf<IncomingReq>()
    var sendStatus by mutableStateOf<String?>(null)
    var currentPeer by mutableStateOf<String?>(null)
    /** 接收弹窗点了"存到…"正在选目录（选完自动接收该请求） */
    var pickingForReceive = false
    /** 应用是否在前台（MainActivity onResume/onPause 维护；
     *  仅后台时发系统通知，前台看着聊天界面就不打扰） */
    var isForeground = false

    /** 仿 QQ/微信的系统通知（横幅+声音；仅后台时发） */
    fun notify(title: String, text: String) {
        if (isForeground) return
        val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE)
                as android.app.NotificationManager
        if (Build.VERSION.SDK_INT >= 26) {
            nm.createNotificationChannel(android.app.NotificationChannel(
                "lt_notify", "消息提醒",
                android.app.NotificationManager.IMPORTANCE_HIGH).apply {
                description = "收到消息或文件时提醒"
            })
        }
        val pi = android.app.PendingIntent.getActivity(ctx, 0,
            Intent(ctx, MainActivity::class.java),
            android.app.PendingIntent.FLAG_UPDATE_CURRENT
                    or android.app.PendingIntent.FLAG_IMMUTABLE)
        val builder = if (Build.VERSION.SDK_INT >= 26)
            android.app.Notification.Builder(ctx, "lt_notify")
        else @Suppress("DEPRECATION") android.app.Notification.Builder(ctx)
        val n = builder
            .setContentTitle(title)
            .setContentText(text)
            .setSmallIcon(android.R.drawable.stat_notify_chat)
            .setContentIntent(pi)
            .setAutoCancel(true)
            .build()
        nm.notify(System.currentTimeMillis().toInt(), n)
    }
    /** 需要"所有文件访问"权限（首次启动 / 权限被收回）→ UI 弹窗引导 */
    var needsAllFilesPermission by mutableStateOf(false)
    /** 当前接收目录（弹窗显示用，选择后立即更新） */
    var saveDirDisplay by mutableStateOf("")
    /** 持久化的默认接收目录（"存到…"的临时目录在批次完成后恢复回它） */
    var defaultSaveDir: String =
        "/storage/emulated/0/Download/LocalTransfer"
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
                notify(peerName, text.take(40))
            }
            override fun onIncoming(req: IncomingReq) {
                notify("${req.peer.name} 想发送文件",
                    "${req.files.size} 个文件 · 点击处理")
                if (autoReceive) {
                    req.decision.complete(true)   // 静默接收，不弹窗不 Toast
                } else {
                    pendingReqs.add(req)
                }
            }
            override fun onProgress(token: String, p: RecvProgress) {
                progress[token] = p.copy(speedBps = sampleSpeed(token, p.transferred))
            }
            override fun onBatchDone(peerId: String, files: List<ReceivedFile>) {
                progress.keys.filter { !it.startsWith("send") }
                    .forEach { progress.remove(it); speedSamples.remove(it) }
                // 位置 = 实际写入的目录（含"存到…"的临时目录），失败时显示原因
                val location = server.customSaveDir
                    ?: files.firstNotNullOfOrNull { it.path }?.let {
                        it.substringBeforeLast('/')
                    } ?: "Download/LocalTransfer"
                val finalLocation = server.publishError ?: location
                val title = if (files.size == 1) files[0].name
                            else "${files.size} 个文件"
                chats.getOrPut(peerId) { mutableStateListOf<ChatEntry>() }.add(ChatEntry.FileCard(
                    false, title, files.sumOf { it.size },
                    System.currentTimeMillis(), finalLocation, files))
                // "存到…"的临时目录只管本批，收完恢复默认（位置已先取出）
                server.customSaveDir = defaultSaveDir
                val names = when (files.size) {
                    1 -> files[0].name
                    in 2..3 -> files.joinToString("、") { it.name }
                    else -> files.take(3).joinToString("、") { it.name } + " 等 ${files.size} 个文件"
                }
                notify("已接收 $title", names)
            }
        })
        me = me.copy(port = server.start(DEFAULT_HTTP_PORT))
        server.setIdentity(me)   // /api/info 返回带真实端口的身份
        // 默认接收目录：Download/LocalTransfer（直接 File 写入）；
        // 用户自定义过的（save_dir）优先。首次启动没权限时由 UI 弹窗引导授权。
        val saved = prefs.getString("save_dir", "")?.ifBlank { null }
        server.customSaveDir = saved ?: "/storage/emulated/0/Download/LocalTransfer"
        defaultSaveDir = server.customSaveDir ?: defaultSaveDir
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

    /** 删除设备：发现表移除+忽略（在线设备不会马上"复活"）；手动列表同步清理 */
    fun removePeer(id: String) {
        GlobalScope.launch(Dispatchers.IO) {
            val ip = disc.removePeer(id)
            main.post {
                if (currentPeer == id) currentPeer = null
                if (ip != null) {
                    ctx.getSharedPreferences("lt", Context.MODE_PRIVATE)
                        .edit().putStringSet("manual_peers",
                            disc.manualIpsSnapshot()).apply()
                }
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

    fun sendPicked(uris: List<Uri>) =
        sendPickedEntries(uris.map { it to queryName(it) })

    /** 发送一批文件（uri → 相对路径；文件夹选择时 rel 带目录前缀） */
    fun sendPickedEntries(entries: List<Pair<Uri, String>>) {
        val peerId = currentPeer ?: return
        val p = peers.value[peerId] ?: return
        GlobalScope.launch(Dispatchers.IO) {
            val sendKey = "send-${System.currentTimeMillis()}"
            val totalLabel = run {
                // 先读文件名+大小（拷贝到缓存，content URI 无法直接二次流式读）
                val metas = mutableListOf<Pair<FileMeta, String>>()
                entries.forEach { (uri, rel) ->
                    val name = rel.substringAfterLast('/')
                    val dst = File(ctx.cacheDir, "${UUID.randomUUID()}_$name")
                    ctx.contentResolver.openInputStream(uri)?.use { ins ->
                        dst.outputStream().use { ins.copyTo(it) }
                    } ?: return@forEach
                    metas.add(FileMeta(UUID.randomUUID().toString(), name, rel,
                        dst.length()) to dst.path)
                }
                metas
            }
            if (totalLabel.isEmpty()) { sendStatus = null; return@launch }
            val metas = totalLabel
            // 卡片标题：单文件=文件名；带目录批次=文件夹名（N 个文件）；否则=N 个文件
            val title = if (metas.size == 1) metas[0].first.name
                else metas.firstOrNull()?.second?.takeIf { it.contains('/') }
                    ?.substringBefore('/')?.ifBlank { null }
                    ?.let { "$it（${metas.size} 个文件）" }
                ?: "${metas.size} 个文件"
            val sum = metas.sumOf { it.first.size }
            // 发送进度卡（消息流里，替代顶部状态条）——
            // 从"等待对方接收"起就带 sending=true（右侧），不再先左后右跳
            main.post {
                progress[sendKey] = RecvProgress(peerId, title, 0, metas.size, 0, sum,
                    sending = true, waiting = true)
            }
            runCatching {
                val ok = api.sendFiles(p, metas) { i, t, tot ->
                    main.post {
                        progress[sendKey] = RecvProgress(peerId, title, i, metas.size,
                            t, tot, sampleSpeed(sendKey, t), sending = true)
                    }
                }
                main.post {
                    progress.remove(sendKey)
                    speedSamples.remove(sendKey)
                    // 等待确认超时（ok=false）静默收场：清进度卡，不出成功卡也不提示
                    if (ok) {
                        // 发送完成的卡片带源文件路径
                        val sent = metas.map { m ->
                            ReceivedFile(m.first.name, m.first.size, null, m.second)
                        }
                        chats.getOrPut(peerId) { mutableStateListOf<ChatEntry>() }.add(ChatEntry.FileCard(
                            true, title, sum, System.currentTimeMillis(), null, sent))
                    }
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

    /** 发送文件夹：SAF 树 uri → DocumentFile 递归收集（rel = 文件夹名/子目录/文件） */
    fun sendFolder(treeUri: Uri) {
        if (currentPeer == null) return
        GlobalScope.launch(Dispatchers.IO) {
            val root = androidx.documentfile.provider.DocumentFile.fromTreeUri(ctx, treeUri)
            val entries = mutableListOf<Pair<Uri, String>>()
            fun walk(d: androidx.documentfile.provider.DocumentFile, prefix: String) {
                d.listFiles().forEach { f ->
                    val n = f.name ?: return@forEach
                    if (f.isDirectory) walk(f, if (prefix.isEmpty()) n else "$prefix/$n")
                    else entries.add(f.uri to (if (prefix.isEmpty()) n else "$prefix/$n"))
                }
            }
            // 树根名字：SAF 的 root.name 在不少 ROM 上是 null → 用文档 id 尾段兜底
            val rootName = root?.name?.takeIf { it.isNotBlank() }
                ?: runCatching {
                    android.provider.DocumentsContract.getTreeDocumentId(treeUri)
                        .substringAfterLast('/')
                }.getOrNull()?.takeIf { it.isNotBlank() }
                ?: "文件夹"
            root?.listFiles()?.forEach { f ->
                val n = f.name ?: return@forEach
                if (f.isDirectory) walk(f, "$rootName/$n")
                else entries.add(f.uri to "$rootName/$n")
            }
            main.post {
                if (entries.isEmpty()) MainActivity.toast(ctx, "该文件夹是空的")
                else sendPickedEntries(entries)
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
        if (App.showGallery) {
            // 应用内相册选择器（微信式）：全屏覆盖，选定即发送
            GalleryPicker(
                onSend = { uris ->
                    App.showGallery = false
                    if (uris.isNotEmpty()) App.sendPicked(uris)
                },
                onClose = { App.showGallery = false },
            )
            return@MaterialTheme
        }
        if (peerId == null) DeviceListScreen() else ChatScreen(peerId)
        // 队头请求先弹，处理完（接收/拒绝/存到）自动弹下一个
        App.pendingReqs.firstOrNull()?.let { req ->
            // 卡片式接收弹窗：渐变图标 + 摘要胶囊 + 拒绝/接收/存到…
            androidx.compose.ui.window.Dialog(
                onDismissRequest = { },
                properties = androidx.compose.ui.window.DialogProperties(
                    dismissOnClickOutside = false)
            ) {
                androidx.compose.material3.Surface(
                    shape = RoundedCornerShape(24.dp),
                    color = MaterialTheme.colorScheme.surface,
                    tonalElevation = 6.dp
                ) {
                    Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(horizontal = 22.dp)
                    ) {
                        Spacer(Modifier.height(22.dp))
                        // 顶部渐变图标
                        Box(
                            modifier = Modifier
                                .size(58.dp)
                                .clip(RoundedCornerShape(19.dp))
                                .background(
                                    androidx.compose.ui.graphics.Brush.linearGradient(
                                        listOf(
                                            MaterialTheme.colorScheme.primary,
                                            MaterialTheme.colorScheme.primary.copy(alpha = .65f)
                                        )
                                    )
                                ),
                            contentAlignment = Alignment.Center
                        ) {
                            Icon(Icons.Rounded.FileDownload, null,
                                tint = androidx.compose.ui.graphics.Color.White,
                                modifier = Modifier.size(30.dp))
                        }
                        Spacer(Modifier.height(12.dp))
                        Text(req.peer.name,
                            fontWeight = FontWeight.Bold, fontSize = 16.sp)
                        Spacer(Modifier.height(2.dp))
                        Text("想发送文件给你", fontSize = 13.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                        // 后面还排着队：处理完这张自动弹下一张
                        if (App.pendingReqs.size > 1) {
                            Spacer(Modifier.height(4.dp))
                            Text("（处理完还有 ${App.pendingReqs.size - 1} 个请求）",
                                fontSize = 11.sp,
                                color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        Spacer(Modifier.height(14.dp))
                        // 文件数 + 大小摘要胶囊
                        androidx.compose.material3.Surface(
                            shape = RoundedCornerShape(20.dp),
                            color = MaterialTheme.colorScheme.primary.copy(alpha = .09f)
                        ) {
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                modifier = Modifier.padding(
                                    horizontal = 12.dp, vertical = 6.dp)
                            ) {
                                Icon(Icons.Rounded.Description, null,
                                    modifier = Modifier.size(14.dp),
                                    tint = MaterialTheme.colorScheme.primary)
                                Spacer(Modifier.width(5.dp))
                                Text("${req.files.size} 个文件 · " +
                                        fmtSize(req.files.sumOf { it.size }),
                                    fontSize = 12.sp,
                                    fontWeight = FontWeight.SemiBold,
                                    color = MaterialTheme.colorScheme.primary)
                            }
                        }
                        Spacer(Modifier.height(18.dp))
                        // 拒绝 / 接收
                        Row(
                            horizontalArrangement = Arrangement.spacedBy(10.dp),
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            androidx.compose.material3.OutlinedButton(
                                onClick = {
                                    req.decision.complete(false)
                                    App.pendingReqs.remove(req)
                                },
                                colors = androidx.compose.material3.ButtonDefaults
                                    .outlinedButtonColors(
                                        contentColor = MaterialTheme.colorScheme.error),
                                border = androidx.compose.foundation.BorderStroke(
                                    1.dp,
                                    MaterialTheme.colorScheme.error.copy(alpha = .35f)),
                                shape = RoundedCornerShape(13.dp),
                                modifier = Modifier.weight(1f)
                            ) {
                                Icon(Icons.Rounded.Close, null,
                                    modifier = Modifier.size(16.dp))
                                Spacer(Modifier.width(4.dp))
                                Text("拒绝")
                            }
                            androidx.compose.material3.Button(
                                onClick = {
                                    // 点接收 → 直接跳进对应会话（看进度）
                                    App.currentPeer = req.peer.id
                                    req.decision.complete(true)
                                    App.pendingReqs.remove(req)
                                },
                                shape = RoundedCornerShape(13.dp),
                                modifier = Modifier.weight(1f)
                            ) {
                                Icon(Icons.Rounded.Download, null,
                                    modifier = Modifier.size(16.dp))
                                Spacer(Modifier.width(4.dp))
                                Text("接收")
                            }
                        }
                        Spacer(Modifier.height(6.dp))
                        // 存到…：选文件夹（授权）后自动接收，保存到所选目录
                        androidx.compose.material3.TextButton(
                            onClick = {
                                App.pickingForReceive = true
                                (ctx as? MainActivity)?.pickDir()
                            },
                            shape = RoundedCornerShape(13.dp),
                            colors = androidx.compose.material3.ButtonDefaults
                                .textButtonColors(
                                    contentColor = MaterialTheme.colorScheme.primary),
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Icon(Icons.AutoMirrored.Rounded.DriveFileMove, null,
                                modifier = Modifier.size(17.dp))
                            Spacer(Modifier.width(5.dp))
                            Text("存到…（选择位置）")
                        }
                        Spacer(Modifier.height(10.dp))
                    }
                }
            }
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
                // 电池优化：后台持续接收的可靠性的关键（厂商省电会杀后台服务）
                val pm = remember {
                    ctx.getSystemService(Context.POWER_SERVICE) as android.os.PowerManager
                }
                var battOptimizedOut by remember {
                    mutableStateOf(pm.isIgnoringBatteryOptimizations(ctx.packageName))
                }
                Row(verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.fillMaxWidth()) {
                    Column(Modifier.weight(1f)) {
                        Text("禁用电池优化", fontSize = 13.sp)
                        Text(
                            if (battOptimizedOut) "已加入白名单，后台接收不受省电限制"
                            else "让后台接收不被系统省电杀掉",
                            fontSize = 11.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    OutlinedButton(onClick = {
                        if (battOptimizedOut) {
                            MainActivity.toast(ctx, "已禁用电池优化")
                        } else {
                            // 返回后重查状态（用户可能点了允许）
                            runCatching {
                                ctx.startActivity(Intent(
                                    android.provider.Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
                                    Uri.parse("package:${ctx.packageName}")))
                            }.onFailure {
                                MainActivity.toast(ctx, "无法打开：${it.message}")
                            }
                        }
                    }) {
                        Text(if (battOptimizedOut) "已禁用" else "去设置",
                            fontSize = 12.sp)
                    }
                }
                // 从系统页返回时刷新状态
                val lifecycleOwner =
                    androidx.compose.ui.platform.LocalLifecycleOwner.current
                DisposableEffect(lifecycleOwner) {
                    val lifecycleObserver = androidx.lifecycle.LifecycleEventObserver { _, event ->
                        if (event == androidx.lifecycle.Lifecycle.Event.ON_RESUME) {
                            battOptimizedOut =
                                pm.isIgnoringBatteryOptimizations(ctx.packageName)
                        }
                    }
                    lifecycleOwner.lifecycle.addObserver(lifecycleObserver)
                    onDispose { lifecycleOwner.lifecycle.removeObserver(lifecycleObserver) }
                }
                Spacer(Modifier.height(12.dp))
                // 允许后台活动（厂商私有页，跳系统设置开启自启/后台权限）
                Row(verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.fillMaxWidth()) {
                    Column(Modifier.weight(1f)) {
                        Text("允许后台活动", fontSize = 13.sp)
                        Text("跳到系统页开启自启动 / 后台运行权限",
                            fontSize = 11.sp,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    OutlinedButton(onClick = {
                        if (!MainActivity.jumpAutoStart(ctx))
                            MainActivity.toast(ctx, "未能打开系统设置页")
                    }) {
                        Text("去开启", fontSize = 12.sp)
                    }
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
@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
fun DeviceRow(p: Peer, onClick: () -> Unit) {
    var confirmDel by remember { mutableStateOf(false) }
    val ctx = LocalContext.current
    Row(
        Modifier.fillMaxWidth()
            .combinedClickable(
                interactionSource = remember {
                    androidx.compose.foundation.interaction.MutableInteractionSource() },
                indication = null,
                onClick = onClick,
                // 长按 = 删除设备（确认弹窗）
                onLongClick = { confirmDel = true },
            )
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
    // 长按 → 删除确认（聊天记录保留；设备重启/重新上线会再出现）
    if (confirmDel) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { confirmDel = false },
            title = { Text("删除设备") },
            text = { Text("从列表移除「${p.info.name}」？聊天记录保留，" +
                    "重启应用后若设备在线会重新出现。") },
            confirmButton = {
                androidx.compose.material3.TextButton(onClick = {
                    App.removePeer(p.info.id)
                    confirmDel = false
                }) { Text("删除", color = androidx.compose.ui.graphics.Color(0xFFEF4444)) }
            },
            dismissButton = {
                androidx.compose.material3.TextButton(onClick = { confirmDel = false }) {
                    Text("取消")
                }
            },
        )
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

/** 底部工具栏入口：图标 + 文字标签 */
@Composable
fun ToolEntry(label: String, icon: androidx.compose.ui.graphics.vector.ImageVector,
              onClick: () -> Unit) {
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = Modifier.clip(RoundedCornerShape(10.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 6.dp),
    ) {
        Icon(icon, label, modifier = Modifier.size(22.dp),
            tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.height(3.dp))
        Text(label, fontSize = 11.sp,
            color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChatScreen(peerId: String) {
    val ctx = LocalContext.current
    val activity = ctx as? MainActivity
    val peers by App.peers.collectAsState()
    val peer = peers[peerId]
    val msgs = remember(peerId) { App.chats.getOrPut(peerId) { mutableStateListOf<ChatEntry>() } }
    var input by remember { mutableStateOf("") }
    var inputOpen by remember { mutableStateOf(false) }
    var showClear by remember { mutableStateOf(false) }
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
                actions = {
                    // 清空当前会话的聊天记录（内存态，不影响已接收文件）
                    IconButton(onClick = { showClear = true }) {
                        Icon(Icons.Rounded.DeleteSweep, "清空记录")
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
            // 消息列表：新消息自动滚到底（用户已滚上去翻历史时不打扰——
            // 只有本来就停在底部附近才跟随）
            val listState = androidx.compose.foundation.lazy.rememberLazyListState()
            val totalRows = msgs.size + progresses.size
            androidx.compose.runtime.LaunchedEffect(totalRows) {
                val info = listState.layoutInfo
                val last = info.visibleItemsInfo.lastOrNull()
                val nearBottom = last == null ||
                    last.index >= totalRows - 2 || info.totalItemsCount == 0
                if (nearBottom && totalRows > 0) {
                    listState.animateScrollToItem(totalRows - 1)
                }
            }
            LazyColumn(Modifier.weight(1f).padding(horizontal = 10.dp),
                state = listState,
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
            if (!inputOpen) {
                // 默认工具栏：四个入口一字排开；点"文本"展开消息输入
                Row(Modifier.fillMaxWidth().padding(vertical = 8.dp),
                    horizontalArrangement = Arrangement.SpaceEvenly) {
                    ToolEntry("图片", Icons.Rounded.Image) {
                        activity?.pickPhotos()
                    }
                    ToolEntry("文件", Icons.Rounded.InsertDriveFile) {
                        activity?.pick()
                    }
                    ToolEntry("文件夹", Icons.Rounded.Folder) {
                        activity?.pickFolder()
                    }
                    ToolEntry("剪贴板", Icons.Rounded.ContentPaste) {
                        val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE)
                                as android.content.ClipboardManager
                        val t = cm.primaryClip?.getItemAt(0)
                            ?.coerceToText(ctx)?.toString()?.trim()
                        if (t.isNullOrEmpty()) MainActivity.toast(ctx, "剪贴板没有文本")
                        else App.sendText(peerId, t)
                    }
                    ToolEntry("文本", Icons.Rounded.Chat) { inputOpen = true }
                }
            } else {
            // 展开的输入条：文件/文件夹/剪贴板 + 胶囊输入框 + 发送/收起
            Row(Modifier.padding(horizontal = 10.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = { activity?.pick() },
                    modifier = Modifier.size(36.dp)) {
                    Icon(Icons.Rounded.AttachFile, "选择文件",
                        modifier = Modifier.size(18.dp),
                        tint = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                // 发送文件夹（SAF 选树，递归带目录结构）
                IconButton(onClick = { activity?.pickFolder() },
                    modifier = Modifier.size(36.dp)) {
                    Icon(Icons.Rounded.Folder, "发送文件夹",
                        modifier = Modifier.size(18.dp),
                        tint = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                // 发送剪贴板文本
                IconButton(onClick = {
                    val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE)
                            as android.content.ClipboardManager
                    val t = cm.primaryClip?.getItemAt(0)
                        ?.coerceToText(ctx)?.toString()?.trim()
                    if (t.isNullOrEmpty()) MainActivity.toast(ctx, "剪贴板没有文本")
                    else App.sendText(peerId, t)
                }, modifier = Modifier.size(36.dp)) {
                    Icon(Icons.Rounded.ContentPaste, "发送剪贴板",
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
                } else {
                    // 收起输入回到工具栏
                    IconButton(onClick = { inputOpen = false },
                        modifier = Modifier.size(36.dp)) {
                        Icon(Icons.Rounded.KeyboardArrowDown, "收起",
                            modifier = Modifier.size(20.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
            }
        }
    }
    // 清空聊天记录确认
    if (showClear) {
        AlertDialog(
            onDismissRequest = { showClear = false },
            title = { Text("清空聊天记录", fontWeight = FontWeight.SemiBold) },
            text = { Text("清空与「${peer?.info?.name ?: "对方"}」的全部消息记录？" +
                    "（不影响已接收的文件）") },
            confirmButton = {
                TextButton(onClick = {
                    msgs.clear()
                    showClear = false
                }) { Text("清空") }
            },
            dismissButton = {
                TextButton(onClick = { showClear = false }) { Text("取消") }
            },
        )
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
                            val now = System.currentTimeMillis()
                            if (e is ChatEntry.Text) {
                                // 双击（350ms 内两次点击）= 直接复制文本
                                if (now - lastTap < 350) {
                                    clip.setText(
                                        androidx.compose.ui.text.AnnotatedString(e.text))
                                    MainActivity.toast(ctx, "已复制")
                                }
                            } else if (e is ChatEntry.FileCard) {
                                // 点击文件卡 = 默认打开：单文件打开文件，
                                // 多文件/文件夹打开保存目录
                                if (e.files.size == 1) {
                                    MainActivity.openFile(ctx, e.files[0])
                                } else {
                                    e.location?.let { loc ->
                                        if (!MainActivity.openDirectory(ctx, loc))
                                            MainActivity.toast(ctx, "请到文件管理器查看：$loc")
                                    }
                                }
                            }
                            lastTap = now
                        },
                        onLongClick = { menu = true },
                    )) {
                when (e) {
                    // 部分复制：SelectionContainer 原生长按选择+拖动手柄+复制工具条
                    // （长按文字=选择；长按气泡边缘 padding=原删除/复制菜单，互不冲突）
                    is ChatEntry.Text ->
                        androidx.compose.foundation.text.selection.SelectionContainer {
                            Text(e.text,
                                color = if (end) androidx.compose.ui.graphics.Color.White
                                else MaterialTheme.colorScheme.onSurface)
                        }
                    // 文件卡只展示，不可点击（用户明确要求）
                    is ChatEntry.FileCard -> Column {
                        // 微信式：单文件图片 → 缩略图直出（点击打开走外层卡片逻辑）
                        if (e.files.size == 1) {
                            val f = e.files[0]
                            val isImg = listOf("png", "jpg", "jpeg", "gif", "webp", "bmp", "heic")
                                .contains(f.name.substringAfterLast('.', "").lowercase())
                            val isVid = listOf("mp4", "mov", "mkv", "webm", "3gp", "m4v")
                                .contains(f.name.substringAfterLast('.', "").lowercase())
                            if ((isImg || isVid) && f.path != null) {
                                val file = java.io.File(f.path)
                                if (file.exists()) {
                                    if (isImg) {
                                        coil.compose.AsyncImage(
                                            model = f.path,
                                            contentDescription = f.name,
                                            modifier = Modifier
                                                .widthIn(max = 220.dp)
                                                .heightIn(max = 280.dp)
                                                .clip(RoundedCornerShape(10.dp)),
                                            contentScale = androidx.compose.ui.layout.ContentScale.FillWidth,
                                        )
                                        Spacer(Modifier.height(2.dp))
                                        Text(fmtSize(e.size), fontSize = 10.sp,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                                        return@Column
                                    } else {
                                        // 视频：渐变底 + 大播放图标的卡片
                                        Box(Modifier
                                            .width(200.dp).height(130.dp)
                                            .clip(RoundedCornerShape(10.dp))
                                            .background(
                                                androidx.compose.ui.graphics.Brush.horizontalGradient(
                                                    listOf(
                                                        androidx.compose.ui.graphics.Color(0xFF1E293B),
                                                        androidx.compose.ui.graphics.Color(0xFF334155)))),
                                            contentAlignment = Alignment.Center) {
                                            Icon(androidx.compose.material.icons.Icons.Rounded.PlayArrow, "视频",
                                                modifier = Modifier.size(44.dp),
                                                tint = androidx.compose.ui.graphics.Color.White)
                                            Text("▶ ${f.name.take(20)}",
                                                fontSize = 10.sp,
                                                color = androidx.compose.ui.graphics.Color.White.copy(alpha = 0.85f),
                                                modifier = Modifier
                                                    .align(Alignment.BottomStart)
                                                    .padding(8.dp),
                                                maxLines = 1, overflow = TextOverflow.Ellipsis)
                                        }
                                        Spacer(Modifier.height(2.dp))
                                        Text(fmtSize(e.size), fontSize = 10.sp,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                                        return@Column
                                    }
                                }
                            }
                        }
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(if (e.files.size > 1 || e.files.isEmpty()) "📁" else "📄",
                                fontSize = 18.sp)
                            Spacer(Modifier.width(8.dp))
                            Text(e.title, maxLines = 1, overflow = TextOverflow.Ellipsis,
                                fontWeight = FontWeight.Medium,
                                color = if (end) androidx.compose.ui.graphics.Color.White
                                else MaterialTheme.colorScheme.onSurface)
                        }
                        // 文件清单（微信式：批内文件名可见，≤4 行+折叠）
                        if (e.files.size > 1) {
                            e.files.take(4).forEach { f ->
                                Text("· ${f.name}", fontSize = 11.sp, maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                    color = if (end) androidx.compose.ui.graphics.Color.White.copy(alpha = 0.85f)
                                    else MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                            if (e.files.size > 4) {
                                Text("… 共 ${e.files.size} 个文件", fontSize = 11.sp,
                                    color = if (end) androidx.compose.ui.graphics.Color.White.copy(alpha = 0.6f)
                                    else MaterialTheme.colorScheme.onSurfaceVariant)
                            }
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
                    leadingIcon = {
                        Icon(Icons.Rounded.ContentCopy, null, Modifier.size(18.dp))
                    },
                    onClick = {
                        clip.setText(
                            androidx.compose.ui.text.AnnotatedString(e.text))
                        MainActivity.toast(ctx, "已复制")
                        menu = false
                    })
            }
            // 文件卡：打开单个文件 / 打开所在目录（接收卡才有保存位置）
            if (e is ChatEntry.FileCard) {
                if (e.files.size == 1) {
                    DropdownMenuItem(
                        text = { Text("打开文件") },
                        leadingIcon = {
                            Icon(Icons.Rounded.InsertDriveFile, null, Modifier.size(18.dp))
                        },
                        onClick = {
                            menu = false
                            MainActivity.openFile(ctx, e.files[0])
                        })
                }
                e.location?.let { loc ->
                    DropdownMenuItem(
                        text = { Text("打开目录") },
                        leadingIcon = {
                            Icon(Icons.Rounded.Folder, null, Modifier.size(18.dp))
                        },
                        onClick = {
                            menu = false
                            if (!MainActivity.openDirectory(ctx, loc))
                                MainActivity.toast(ctx, "请到文件管理器查看：$loc")
                        })
                }
            }
            DropdownMenuItem(
                text = { Text("删除") },
                leadingIcon = {
                    Icon(Icons.Rounded.Delete, null, Modifier.size(18.dp),
                        tint = androidx.compose.ui.graphics.Color(0xFFDC2626))
                },
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
