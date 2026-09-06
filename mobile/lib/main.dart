/// LocalTransfer 移动客户端入口：设备列表页 + 会话页。
/// 与桌面端（Rust+GPUI）通过 UDP 多播发现 + HTTP 互通。
library;

import 'dart:async';
import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
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

/// 全局应用状态（发现 + 服务 + 会话）
class AppState {
  late final DeviceInfo me;
  late final Discovery disc;
  late final TransferApi api;
  late final AppServer server;
  final Map<String, List<ChatMsg>> chats = {};
  final Map<String, String> peerNames = {}; // id → name（离线也保留）
  final Map<String, TransferProgress> liveProgress = {}; // "idx-transferred"…发送进度按会话展示
  bool ready = false;

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
      onProgress: (peerId, peerName, fileName, total, transferred, done, path) {
        chats.putIfAbsent(peerId, () => []).add(ChatMsg(
            fileName: fileName,
            fileSize: total,
            outgoing: false,
            atMs: DateTime.now().millisecondsSinceEpoch));
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

  /// 手动添加设备（发现不通时按 IP 直连）
  Future<void> _addManual() async {
    final c = TextEditingController();
    final err = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('添加设备'),
        content: TextField(
          controller: c,
          autofocus: true,
          keyboardType: TextInputType.url,
          decoration:
              const InputDecoration(hintText: '192.168.1.5 或 192.168.1.5:17878'),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('取消')),
          FilledButton(
              onPressed: () => Navigator.pop(ctx, c.text.trim()),
              child: const Text('连接')),
        ],
      ),
    );
    if (err == null || err.isEmpty) return;
    final fail = await app.disc.addManual(err);
    if (!mounted) return;
    if (fail != null) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(fail)));
    }
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
            icon: const Icon(Icons.add),
            tooltip: '手动添加设备（IP 直连）',
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
  late final Peer _peer; // 进入会话时的快照（设备超时移除后仍可安全渲染）

  Peer get peer => _peer;
  String get peerName => _peer.info.name;

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
                              ? Row(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    const Icon(Icons.insert_drive_file, size: 18),
                                    const SizedBox(width: 6),
                                    Flexible(
                                        child: Column(
                                      crossAxisAlignment: CrossAxisAlignment.start,
                                      children: [
                                        Text(m.fileName,
                                            overflow: TextOverflow.ellipsis),
                                        Text(fmtSize(m.fileSize),
                                            style: const TextStyle(fontSize: 11)),
                                      ],
                                    )),
                                  ],
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
