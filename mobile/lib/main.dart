/// LocalTransfer 移动客户端入口：设备列表页 + 会话页。
/// 与桌面端（Rust+GPUI）通过 UDP 多播发现 + HTTP 互通。
library;

import 'dart:async';
import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:mobile_scanner/mobile_scanner.dart';
import 'package:open_filex/open_filex.dart';
import 'package:qr_flutter/qr_flutter.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:uuid/uuid.dart';

import 'api.dart';
import 'discovery.dart';
import 'proto.dart';
import 'server.dart';

void main() {
  runApp(const LocalTransferApp());
}

class LocalTransferApp extends StatelessWidget {
  const LocalTransferApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'LocalTransfer',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(colorSchemeSeed: const Color(0xFF6366F1), useMaterial3: true),
      darkTheme: ThemeData(
          colorSchemeSeed: const Color(0xFF6366F1),
          brightness: Brightness.dark,
          useMaterial3: true),
      home: const HomePage(),
    );
  }
}

/// 全局应用状态（发现 + 服务 + 会话）——接收进度变化时通知 UI（节流）
class AppState extends ChangeNotifier {
  late final DeviceInfo me;
  late final Discovery disc;
  late final TransferApi api;
  late final AppServer server;
  final Map<String, List<ChatMsg>> chats = {};
  final Map<String, String> peerNames = {}; // id → name（离线也保留）
  final Map<String, RecvProgress> recvProgress = {}; // token → 进度
  DateTime _lastNotify = DateTime.fromMillisecondsSinceEpoch(0);
  bool ready = false;

  void _notifyThrottled() {
    final now = DateTime.now();
    if (now.difference(_lastNotify).inMilliseconds < 100) return;
    _lastNotify = now;
    notifyListeners();
  }

  Future<void> init() async {
    final prefs = await SharedPreferences.getInstance();
    var id = prefs.getString('device_id');
    if (id == null) {
      id = const Uuid().v4();
      await prefs.setString('device_id', id);
    }
    final name = prefs.getString('device_name') ??
        '手机-${id.substring(0, 4).toUpperCase()}';
    me = DeviceInfo(
      id: id,
      name: name,
      plat: Platform.isAndroid ? 'android' : 'ios',
      port: 0, // 服务启动后回填
    );
    api = TransferApi(me);
    server = AppServer(
      me: me,
      onMessage: (msg, peerId) => chats.putIfAbsent(peerId, () => []).add(msg),
      onIncoming: (req) => _pendingIncoming.add(req),
      // 接收进度 → 全局表（UI 节流刷新）
      onRecvProgress: (token, p) {
        recvProgress[token] = p;
        _notifyThrottled();
      },
      onRecvEnd: (token) {
        recvProgress.remove(token);
        notifyListeners();
      },
      // 整批（一次会话）完成 → 合并为一张卡片：文件夹名或"N 个文件"
      onBatchDone: (peerId, peer, files, saveDir) {
        final folder = _folderName(files);
        final target = _openTarget(files);
        final location = _location(files);
        chats.putIfAbsent(peerId, () => []).add(ChatMsg(
            fileName: folder,
            fileSize: files.fold<int>(0, (s, f) => s + f.size),
            outgoing: false,
            atMs: DateTime.now().millisecondsSinceEpoch,
            path: target,
            location: location));
      },
    );
    final port = await server.start();
    me.port = port; // announce 循环从这之后才开始，端口已就绪
    disc = Discovery(me);
    await disc.restoreManual(); // 恢复手动添加过的设备（TCP 保活立即接管）
    await disc.start();
    ready = true;
  }
}

/// 全局单例（简单起见）
final app = AppState();

/// 批次卡片的显示名：全部文件在同一文件夹下 → 文件夹名；否则"N 个文件"
String _folderName(List<DoneFile> files) {
  if (files.length == 1) {
    final rel = files[0].relPath;
    final i = rel.indexOf('/');
    return i > 0 ? rel.substring(0, i) : rel;
  }
  String? folder;
  for (final f in files) {
    final i = f.relPath.indexOf('/');
    if (i <= 0) return '${files.length} 个文件'; // 散文件混入
    final first = f.relPath.substring(0, i);
    if (folder == null) {
      folder = first;
    } else if (folder != first) {
      return '${files.length} 个文件';
    }
  }
  return folder ?? '${files.length} 个文件';
}

/// 卡片点击的打开目标（约定前缀 uri:/path:/folder:）：
/// 单文件：uri:URI|relPath|publicPath（URI 打开失败回退真实路径）
/// 多文件：folder:RelPath（文件管理器定位；失败回退下载列表）
String _openTarget(List<DoneFile> files) {
  if (files.length == 1) {
    final f = files[0];
    if (f.uri != null) {
      return 'uri:${f.uri}|${f.relPath}|${f.publicPath ?? ''}';
    }
    if (f.publicPath != null) return 'path:${f.publicPath}';
    return 'folder:LocalTransfer';
  }
  String? folder;
  for (final f in files) {
    final i = f.relPath.indexOf('/');
    if (i <= 0) return 'folder:LocalTransfer';
    final first = f.relPath.substring(0, i);
    if (folder == null) {
      folder = first;
    } else if (folder != first) {
      return 'folder:LocalTransfer';
    }
  }
  return 'folder:LocalTransfer/$folder';
}

/// 卡片上显示的保存位置（人类可读）
String _location(List<DoneFile> files) {
  final target = _openTarget(files);
  if (target.startsWith('uri:')) {
    final rel = target.split('|').last;
    return 'Download/LocalTransfer/$rel';
  }
  if (target.startsWith('folder:')) {
    return 'Download/${target.substring(7)}';
  }
  return 'Download/LocalTransfer';
}

/// 待确认的接收请求流（UI 弹卡片确认）
final _pendingIncoming = StreamController<IncomingReq>.broadcast();

class HomePage extends StatefulWidget {
  const HomePage({super.key});
  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage> with WidgetsBindingObserver {
  String? _err;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    app.init().then((_) {
      if (mounted) setState(() {});
    }).catchError((e) {
      if (mounted) setState(() => _err = '$e');
    });
    // 桌面端发来的接收请求 → 弹确认
    _pendingIncoming.stream.listen(_showIncoming);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // 进程退出路径（退出即广播 bye，桌面端立刻判离线；被系统强杀时
    // 由 15s 心跳超时兜底）
    if (state == AppLifecycleState.detached) {
      app.disc.shutdown();
    }
  }

  /// 添加设备：扫码 或 手动输入 IP
  Future<void> _addManual() async {
    final c = TextEditingController();
    final choice = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('添加设备'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text('方式一：在电脑端点头像显示二维码，手机扫码',
                style: TextStyle(fontSize: 13)),
            const SizedBox(height: 8),
            FilledButton.icon(
              icon: const Icon(Icons.qr_code_scanner),
              label: const Text('扫码添加'),
              onPressed: () => Navigator.pop(ctx, '__scan__'),
            ),
            const Divider(height: 24),
            const Text('方式二：手动输入地址', style: TextStyle(fontSize: 13)),
            const SizedBox(height: 8),
            TextField(
              controller: c,
              keyboardType: TextInputType.url,
              decoration: const InputDecoration(
                  hintText: '192.168.1.5 或 192.168.1.5:17878',
                  isDense: true),
            ),
          ],
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('取消')),
          FilledButton(
              onPressed: () => Navigator.pop(ctx, c.text.trim()),
              child: const Text('连接')),
        ],
      ),
    );
    if (choice == null || choice.isEmpty) return;
    var addr = choice;
    if (choice == '__scan__') {
      final scanned = await Navigator.push<String>(
        context,
        MaterialPageRoute(builder: (_) => const ScannerPage()),
      );
      if (scanned == null || scanned.isEmpty) return;
      addr = scanned;
    }
    final fail = await app.disc.addManual(addr);
    if (!mounted) return;
    if (fail != null) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(fail)));
    }
  }

  /// 本机二维码（其他设备扫码添加本机）
  Future<void> _showMyQr() async {
    final ip = await Discovery.myLanIp();
    await showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('本机二维码'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              color: Colors.white,
              padding: const EdgeInsets.all(12),
              child: QrImageView(
                data: 'http://$ip:${app.me.port}',
                size: 200,
              ),
            ),
            const SizedBox(height: 8),
            Text('$ip:${app.me.port}',
                style: const TextStyle(fontSize: 13)),
          ],
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('关闭')),
        ],
      ),
    );
  }

  void _showIncoming(IncomingReq req) {
    final total = req.files.fold<int>(0, (s, f) => s + f.size);
    showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (ctx) => AlertDialog(
        title: Text('${req.peer.name} 想发送文件'),
        content: Text('${req.files.length} 个文件 · ${fmtSize(total)}'),
        actions: [
          TextButton(
            onPressed: () {
              req.decision.complete(false);
              Navigator.pop(ctx);
            },
            child: const Text('拒绝'),
          ),
          FilledButton(
            onPressed: () {
              req.decision.complete(true);
              Navigator.pop(ctx);
              setState(() {});
            },
            child: const Text('接收'),
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    if (_err != null) {
      return Scaffold(body: Center(child: Text('启动失败：$_err')));
    }
    if (!app.ready) {
      return const Scaffold(body: Center(child: CircularProgressIndicator()));
    }
    return Scaffold(
      appBar: AppBar(
        title: Text(app.me.name),
        actions: [
          IconButton(
            icon: const Icon(Icons.qr_code),
            tooltip: '本机二维码',
            onPressed: _showMyQr,
          ),
          IconButton(
            icon: const Icon(Icons.add),
            tooltip: '添加设备（扫码或 IP 直连）',
            onPressed: _addManual,
          ),
          IconButton(
            icon: const Icon(Icons.edit),
            tooltip: '改名',
            onPressed: () async {
              final c = TextEditingController(text: app.me.name);
              final name = await showDialog<String>(
                context: context,
                builder: (ctx) => AlertDialog(
                  title: const Text('设备名'),
                  content: TextField(controller: c, autofocus: true),
                  actions: [
                    TextButton(
                        onPressed: () => Navigator.pop(ctx),
                        child: const Text('取消')),
                    FilledButton(
                        onPressed: () => Navigator.pop(ctx, c.text.trim()),
                        child: const Text('确定')),
                  ],
                ),
              );
              if (name != null && name.isNotEmpty) {
                final prefs = await SharedPreferences.getInstance();
                await prefs.setString('device_name', name);
                app.me.name = name; // 立即生效：下一条 announce 即带新名
                if (mounted) setState(() {});
              }
            },
          ),
        ],
      ),
      body: ListenableBuilder(
        listenable: app.disc,
        builder: (context, _) {
          final peers = app.disc.peers.values.where((p) => p.online(deviceTimeoutSecs)).toList()
            ..sort((a, b) => a.info.name.compareTo(b.info.name));
          final list = peers.isEmpty
              ? const Center(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(Icons.wifi_find, size: 48, color: Colors.grey),
                      SizedBox(height: 12),
                      Text('等待设备上线…', style: TextStyle(color: Colors.grey)),
                      Text('需与电脑在同一局域网', style: TextStyle(color: Colors.grey, fontSize: 12)),
                    ],
                  ),
                )
              : ListView(
                  children: peers
                      .map((p) => ListTile(
                            leading: CircleAvatar(
                              backgroundColor:
                                  Theme.of(context).colorScheme.primaryContainer,
                              child: Text(p.info.name.characters.first),
                            ),
                            title: Text(p.info.name),
                            subtitle: Text(
                                '${p.info.plat} · ${p.addr.address}:${p.info.port}',
                                style: const TextStyle(fontSize: 12)),
                            trailing: p.manual
                                ? const Icon(Icons.link, size: 16, color: Colors.grey)
                                : null,
                            onTap: () => Navigator.push(
                                  context,
                                  MaterialPageRoute(
                                      builder: (_) => ChatPage(peerId: p.info.id)),
                                ),
                          ))
                      .toList(),
                );
          // 诊断栏：排查"看不到对方"类问题（收包计数、锁状态、构建版本）
          final d = app.disc;
          return Column(
            children: [
              Expanded(child: list),
              Container(
                width: double.infinity,
                padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
                color: Theme.of(context)
                    .colorScheme
                    .surfaceContainerHighest
                    .withValues(alpha: 0.5),
                child: Text(
                  'b0906e · 收包 ${d.rxPackets} · 发现包 ${d.rxAnnounces} · 发包 ${d.txPackets}'
                  ' · 多播锁 ${d.multicastLockOk ? "√" : "×"}'
                  '${d.lastRxFrom.isEmpty ? "" : " · 最近来源 ${d.lastRxFrom}"}',
                  style: const TextStyle(fontSize: 10, color: Colors.grey),
                ),
              ),
            ],
          );
        },
      ),
    );
  }
}

/// 扫码添加设备页（扫电脑端头像点出的二维码）
class ScannerPage extends StatefulWidget {
  const ScannerPage({super.key});
  @override
  State<ScannerPage> createState() => _ScannerPageState();
}

class _ScannerPageState extends State<ScannerPage> {
  final MobileScannerController _ctrl = MobileScannerController();
  bool _done = false;

  @override
  void dispose() {
    _ctrl.dispose();
    super.dispose();
  }

  void _onDetect(BarcodeCapture cap) {
    if (_done) return;
    final raw = cap.barcodes.firstOrNull?.rawValue;
    if (raw == null || raw.isEmpty) return;
    _done = true;
    Navigator.pop(context, raw);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('扫码添加设备')),
      body: Stack(
        children: [
          MobileScanner(
            controller: _ctrl,
            onDetect: _onDetect,
          ),
          const Align(
            alignment: Alignment.bottomCenter,
            child: Padding(
              padding: EdgeInsets.all(24),
              child: Text(
                '对准电脑端头像二维码（http://IP:端口）',
                style: TextStyle(color: Colors.white, fontSize: 13),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class ChatPage extends StatefulWidget {
  final String peerId;
  const ChatPage({super.key, required this.peerId});
  @override
  State<ChatPage> createState() => _ChatPageState();
}

class _ChatPageState extends State<ChatPage> {
  final _input = TextEditingController();
  bool _sending = false;
  String? _status;
  late final Peer _peer; // 进入会话时的快照（设备被清理后兜底渲染）

  // 实时查表（TCP 保活持续刷新 lastSeen；用快照会导致"永远离线"）
  Peer get peer => app.disc.peers[widget.peerId] ?? _peer;
  String get peerName => peer.info.name;

  @override
  void initState() {
    super.initState();
    _peer = app.disc.peers[widget.peerId]!;
    app.disc.addListener(_refresh);
  }

  @override
  void dispose() {
    app.disc.removeListener(_refresh);
    _input.dispose();
    super.dispose();
  }

  void _refresh() {
    if (mounted) setState(() {});
  }

  Future<void> _send() async {
    final text = _input.text.trim();
    if (text.isEmpty) return;
    _input.clear();
    setState(() {
      app.chats.putIfAbsent(widget.peerId, () => []).add(ChatMsg(
          text: text, outgoing: true, atMs: DateTime.now().millisecondsSinceEpoch));
    });
    try {
      await app.api.sendText(peer, text);
    } catch (e) {
      if (mounted) {
        setState(() => _status = '$e');
      }
    }
  }

  /// 打开接收到的文件/文件夹：
  /// 单文件 → 内容 URI（失败回退真实路径）；文件夹 → 文件管理器定位（失败回退下载列表）
  Future<void> _openFile(ChatMsg m) async {
    final t = m.path;
    if (t == null) return;
    const ch = MethodChannel('localtransfer/downloads');
    try {
      if (t.startsWith('uri:')) {
        final parts = t.substring(4).split('|');
        final ext = parts.length > 1
            ? parts[1].split('.').lastOrNull?.toLowerCase()
            : null;
        try {
          await ch.invokeMethod(
              'openUri', {'uri': parts[0], 'mime': _mimeOf(ext)});
          return;
        } on PlatformException {
          // 回退：真实路径打开
          final p = parts.length > 2 ? parts[2] : '';
          if (p.isNotEmpty) {
            final r = await OpenFilex.open(p);
            if (r.type == ResultType.done) return;
          }
          rethrow;
        }
      } else if (t.startsWith('folder:')) {
        await ch.invokeMethod('openFolder', {'rel': t.substring(7)});
      } else if (t.startsWith('path:')) {
        final r = await OpenFilex.open(t.substring(5));
        if (r.type != ResultType.done && mounted) {
          ScaffoldMessenger.of(context)
              .showSnackBar(SnackBar(content: Text('打开失败：${r.message}')));
        }
      }
    } on PlatformException catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text('打开失败：${e.message ?? '无可用应用'}')));
      }
    }
  }

  static String _mimeOf(String? ext) {
    switch (ext) {
      case 'jpg':
      case 'jpeg':
        return 'image/jpeg';
      case 'png':
        return 'image/png';
      case 'gif':
        return 'image/gif';
      case 'webp':
        return 'image/webp';
      case 'mp4':
        return 'video/mp4';
      case 'mp3':
        return 'audio/mpeg';
      case 'pdf':
        return 'application/pdf';
      case 'txt':
      case 'md':
      case 'log':
        return 'text/plain';
      default:
        return 'application/octet-stream';
    }
  }

  Future<void> _pickAndSend() async {
    final picked = await FilePicker.platform.pickFiles(allowMultiple: true);
    if (picked == null || picked.files.isEmpty) return;
    final files = <(FileMeta, String)>[];
    for (final f in picked.files) {
      if (f.path == null) continue;
      files.add((FileMeta(
        id: const Uuid().v4(),
        name: f.name,
        relPath: f.name,
        size: f.size,
      ), f.path!));
    }
    if (files.isEmpty) return;
    setState(() {
      _sending = true;
      _status = null;
      app.chats.putIfAbsent(widget.peerId, () => []).addAll(files
          .map((f) => ChatMsg(
              fileName: f.$1.name,
              fileSize: f.$1.size,
              outgoing: true,
              atMs: DateTime.now().millisecondsSinceEpoch)));
    });
    try {
      await app.api.sendFiles(peer, files, (idx, transferred, total) {
        if (mounted) {
          setState(() => _status = '发送 ${idx + 1}/${files.length}：'
              '${(transferred / (total > 0 ? total : 1) * 100).toStringAsFixed(0)}%');
        }
      });
      if (mounted) setState(() => _status = null);
    } catch (e) {
      if (mounted) setState(() => _status = '$e');
    } finally {
      if (mounted) setState(() => _sending = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final msgs = app.chats[widget.peerId] ?? const <ChatMsg>[];
    return Scaffold(
      appBar: AppBar(
        title: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(peerName, style: const TextStyle(fontSize: 16)),
            Text(
              peer.online(deviceTimeoutSecs) ? '在线' : '离线',
              style: TextStyle(
                  fontSize: 12,
                  color: peer.online(deviceTimeoutSecs)
                      ? Colors.green
                      : Colors.grey),
            ),
          ],
        ),
      ),
      body: Column(
        children: [
          // 接收进度条（进行中的接收会话）
          ListenableBuilder(
            listenable: app,
            builder: (context, _) {
              final rp = app.recvProgress.values
                  .where((p) => p.peerId == widget.peerId)
                  .toList();
              if (rp.isEmpty) return const SizedBox.shrink();
              final p = rp.first;
              final frac = p.total > 0 ? p.transferred / p.total : 0.0;
              return Container(
                width: double.infinity,
                padding:
                    const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
                color: Theme.of(context)
                    .colorScheme
                    .surfaceContainerHighest
                    .withValues(alpha: 0.5),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      '接收 ${p.label}（${p.fileIdx + 1}/${p.fileCount}）'
                      ' · ${(frac * 100).toStringAsFixed(0)}%',
                      style: const TextStyle(fontSize: 12),
                    ),
                    const SizedBox(height: 4),
                    LinearProgressIndicator(value: frac.clamp(0.0, 1.0)),
                  ],
                ),
              );
            },
          ),
          if (_status != null)
            Container(
              width: double.infinity,
              color: Theme.of(context).colorScheme.surfaceContainerHighest,
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
              child: Text(_status!, style: const TextStyle(fontSize: 12)),
            ),
          Expanded(
            child: msgs.isEmpty
                ? const Center(
                    child: Text('发送文本，或点 📎 选择文件',
                        style: TextStyle(color: Colors.grey)))
                : ListView.builder(
                    padding: const EdgeInsets.all(12),
                    itemCount: msgs.length,
                    itemBuilder: (context, i) {
                      final m = msgs[i];
                      return Align(
                        alignment: m.outgoing
                            ? Alignment.centerRight
                            : Alignment.centerLeft,
                        child: Container(
                          margin: const EdgeInsets.symmetric(vertical: 4),
                          padding: const EdgeInsets.symmetric(
                              horizontal: 12, vertical: 8),
                          constraints: const BoxConstraints(maxWidth: 300),
                          decoration: BoxDecoration(
                            color: m.outgoing
                                ? Theme.of(context).colorScheme.primary
                                : Theme.of(context)
                                    .colorScheme
                                    .surfaceContainerHighest,
                            borderRadius: BorderRadius.circular(14),
                          ),
                          child: m.isFile
                              ? GestureDetector(
                                  onTap: () => _openFile(m),
                                  child: Row(
                                    mainAxisSize: MainAxisSize.min,
                                    children: [
                                      Icon(
                                          (m.path ?? '').startsWith('folder:')
                                              ? Icons.folder
                                              : Icons.insert_drive_file,
                                          size: 18),
                                      const SizedBox(width: 6),
                                      Flexible(
                                          child: Column(
                                        crossAxisAlignment: CrossAxisAlignment.start,
                                        children: [
                                          Text(m.fileName,
                                              overflow: TextOverflow.ellipsis),
                                          if (m.location != null)
                                            Text(m.location!,
                                                overflow: TextOverflow.ellipsis,
                                                style: const TextStyle(
                                                    fontSize: 10,
                                                    color: Colors.grey)),
                                          Text(
                                              m.path != null
                                                  ? '${fmtSize(m.fileSize)} · 点击打开'
                                                  : fmtSize(m.fileSize),
                                              style:
                                                  const TextStyle(fontSize: 11)),
                                        ],
                                      )),
                                    ],
                                  ),
                                )
                              : Text(
                                  m.text,
                                  style: TextStyle(
                                      color: m.outgoing
                                          ? Theme.of(context)
                                              .colorScheme
                                              .onPrimary
                                          : null),
                                ),
                        ),
                      );
                    },
                  ),
          ),
          SafeArea(
            child: Padding(
              padding: const EdgeInsets.all(8),
              child: Row(
                children: [
                  IconButton(
                    icon: const Icon(Icons.attach_file),
                    onPressed: _sending ? null : _pickAndSend,
                    tooltip: '发送文件',
                  ),
                  Expanded(
                    child: TextField(
                      controller: _input,
                      minLines: 1,
                      maxLines: 4,
                      onSubmitted: (_) => _send(),
                      decoration: const InputDecoration(
                        hintText: '输入消息',
                        border: OutlineInputBorder(),
                        isDense: true,
                      ),
                    ),
                  ),
                  const SizedBox(width: 8),
                  FilledButton(
                    onPressed: _sending ? null : _send,
                    child: const Text('发送'),
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}
