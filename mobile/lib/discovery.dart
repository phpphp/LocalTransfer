/// UDP 多播发现 —— 与桌面端 discovery.rs 同协议。
/// - 加入多播组监听 announce / bye
/// - 周期性多播自己的 announce（桌面端收到会单播回它的 announce，完成互发现）
/// - 收到别人的 announce 时单播回自己（2 秒节流防风暴）
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:http/http.dart' as http;

import 'proto.dart';

/// 注册表里的一台设备
class Peer {
  final DeviceInfo info;
  final InternetAddress addr;
  DateTime lastSeen;
  /// 手动添加的设备不参与超时清理（发现不通时的直连兜底）
  final bool manual;
  Peer({
    required this.info,
    required this.addr,
    required this.lastSeen,
    this.manual = false,
  });
  String get httpBase => 'http://${addr.address}:${info.port}';
  bool online(int timeoutSecs) =>
      manual || DateTime.now().difference(lastSeen).inSeconds < timeoutSecs;
}

class Discovery extends ChangeNotifier {
  final DeviceInfo me;
  final Map<String, Peer> peers = {};
  RawDatagramSocket? _sock;
  Timer? _tick;
  Timer? _prune;
  final _ctrl = StreamController<Peer>.broadcast();
  /// 每次设备上线/信息变化发一条
  Stream<Peer> get onPeerUp => _ctrl.stream;
  final Map<String, DateTime> _lastUnicastReply = {};
  // 诊断计数（列表页底部显示，排查发现问题时看）
  int rxPackets = 0;
  int txPackets = 0;
  int rxAnnounces = 0;
  String lastRxFrom = '';
  bool multicastLockOk = false;

  Discovery(this.me);

  Future<void> start() async {
    // Android：WiFi 驱动默认丢弃多播/广播帧，必须持 MulticastLock
    //（平台通道在 MainActivity.kt 实现；其他平台静默跳过）
    try {
      await const MethodChannel('localtransfer/multicast').invokeMethod('acquire');
      multicastLockOk = true;
    } on PlatformException {
      multicastLockOk = false;
    } on MissingPluginException {
      multicastLockOk = false; // 桌面/测试环境
    }
    final sock = await RawDatagramSocket.bind(
        InternetAddress.anyIPv4, discoveryPort,
        reuseAddress: true, reusePort: false);
    try {
      // dart:io 的多播加入是同步方法 joinMulticast
      sock.joinMulticast(InternetAddress(discoveryGroup));
    } catch (e) {
      // 部分网络/模拟器不允许 join：仍可广播+单播兜底
      // ignore: avoid_print
      print('joinMulticast 失败（继续广播兜底）: $e');
    }
    sock.multicastLoopback = false;
    _sock = sock;
    sock.listen((event) {
      if (event != RawSocketEvent.read) return;
      final dg = sock.receive();
      if (dg == null) return;
      rxPackets++;
      _onPacket(String.fromCharCodes(dg.data), dg.address);
    });
    // 立即宣布一次 + 每 5s 心跳（15s 超时下允许丢 2 个周期）
    _announce();
    _tick = Timer.periodic(const Duration(seconds: 5), (_) => _announce());
    // 离线清理（2s 一查，手动添加的设备除外）
    _prune = Timer.periodic(const Duration(seconds: 2), _pruneOffline);
  }

  /// 手动添加设备（发现不通时直连兜底）：探测 /api/info 成则入表
  Future<String?> addManual(String hostPort) {
    return Future(() async {
      var hp = hostPort.trim();
      int port = defaultHttpPort;
      if (hp.contains(':')) {
        final parts = hp.split(':');
        hp = parts[0];
        port = int.tryParse(parts[1]) ?? defaultHttpPort;
      }
      if (hp.isEmpty) return '请输入 IP';
      // 复用 API 客户端探测（需要独立实例避免循环依赖）
      final r = await http
          .get(Uri.parse('http://$hp:$port/api/info'))
          .timeout(const Duration(seconds: 3));
      if (r.statusCode != 200) return '对方返回 ${r.statusCode}';
      final info = DeviceInfo.fromJson(jsonDecode(r.body) as Map<String, dynamic>);
      if (info.v != protocolVersion) return '协议版本不兼容（对方 v${info.v}）';
      peers[info.id] = Peer(
        info: info,
        addr: InternetAddress(hp),
        lastSeen: DateTime.now(),
        manual: true,
      );
      notifyListeners();
      return null; // 成功
    });
  }

  void _onPacket(String raw, InternetAddress from) {
    final (kind, info) = decodeAnnounce(raw);
    if (kind == 'bye') {
      final id = (jsonDecode(raw) as Map<String, dynamic>)['id'] as String?;
      if (id != null && peers.remove(id) != null) notifyListeners();
      return;
    }
    if (kind != 'announce' || info == null) return;
    if (info.id == me.id || info.v != protocolVersion) return;
    rxAnnounces++;
    lastRxFrom = from.address;
    final existed = peers[info.id];
    peers[info.id] = Peer(info: info, addr: from, lastSeen: DateTime.now());
    // 单播回自己的 announce（节流）
    final last = _lastUnicastReply[info.id];
    if (last == null || DateTime.now().difference(last).inSeconds >= 2) {
      _lastUnicastReply[info.id] = DateTime.now();
      _sock?.send(utf8.encode(jsonEncode(me.toAnnounce())), from, discoveryPort);
      txPackets++;
    }
    if (existed == null || existed.info != info || !existed.online(deviceTimeoutSecs)) {
      _ctrl.add(peers[info.id]!);
    }
    notifyListeners();
  }

  void _pruneOffline(Timer _) {
    final before = peers.length;
    peers.removeWhere((_, p) =>
        !p.manual &&
        DateTime.now().difference(p.lastSeen).inSeconds >= deviceTimeoutSecs);
    if (peers.length != before) notifyListeners();
  }

  void _announce() {
    final data = utf8.encode(jsonEncode(me.toAnnounce()));
    // 多播
    _sock?.send(data, InternetAddress(discoveryGroup), discoveryPort);
    // 广播兜底（部分 AP 不转发多播；桌面端同样在发现口监听广播）
    try {
      _sock?.send(data, InternetAddress('255.255.255.255'), discoveryPort);
    } catch (_) {}
    txPackets += 2;
  }

  /// 退出时广播 bye（桌面端会立即把本机标记离线）
  Future<void> shutdown() async {
    _tick?.cancel();
    _prune?.cancel();
    try {
      final data = utf8.encode(jsonEncode({'t': 'bye', 'id': me.id}));
      _sock?.send(data, InternetAddress(discoveryGroup), discoveryPort);
      _sock?.send(data, InternetAddress('255.255.255.255'), discoveryPort);
    } catch (_) {}
    _sock?.close(); // dart:io 的 close 是同步 void
    _sock = null;
    try {
      await const MethodChannel('localtransfer/multicast').invokeMethod('release');
    } catch (_) {}
  }
}
