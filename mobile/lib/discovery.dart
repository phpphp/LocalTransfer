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

import 'proto.dart';

/// 注册表里的一台设备
class Peer {
  final DeviceInfo info;
  final InternetAddress addr;
  DateTime lastSeen;
  Peer({required this.info, required this.addr, required this.lastSeen});
  String get httpBase => 'http://${addr.address}:${info.port}';
  bool online(int timeoutSecs) =>
      DateTime.now().difference(lastSeen).inSeconds < timeoutSecs;
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

  Discovery(this.me);

  Future<void> start() async {
    // Android：WiFi 驱动默认丢弃多播/广播帧，必须持 MulticastLock
    //（平台通道在 MainActivity.kt 实现；其他平台静默跳过）
    try {
      await const MethodChannel('localtransfer/multicast').invokeMethod('acquire');
    } on PlatformException {
      // 非 Android 平台无此通道
    } on MissingPluginException {
      // 桌面/测试环境
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
      _onPacket(String.fromCharCodes(dg.data), dg.address);
    });
    // 立即宣布一次 + 周期心跳（前 10s 每 2s，之后每 10s，与桌面端节奏一致）
    _announce();
    var tick = 0;
    _tick = Timer.periodic(const Duration(seconds: 5), (_) {
      tick++;
      if (tick <= 2 || tick % 2 == 0) _announce();
    });
    // 离线清理
    _prune = Timer.periodic(const Duration(seconds: 5), _pruneOffline);
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
    final existed = peers[info.id];
    peers[info.id] = Peer(info: info, addr: from, lastSeen: DateTime.now());
    // 单播回自己的 announce（节流）
    final last = _lastUnicastReply[info.id];
    if (last == null || DateTime.now().difference(last).inSeconds >= 2) {
      _lastUnicastReply[info.id] = DateTime.now();
      _sock?.send(utf8.encode(jsonEncode(me.toAnnounce())), from, discoveryPort);
    }
    if (existed == null || existed.info != info || !existed.online(deviceTimeoutSecs)) {
      _ctrl.add(peers[info.id]!);
    }
    notifyListeners();
  }

  void _pruneOffline(Timer _) {
    final before = peers.length;
    peers.removeWhere(
        (_, p) => DateTime.now().difference(p.lastSeen).inSeconds >= deviceTimeoutSecs);
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
