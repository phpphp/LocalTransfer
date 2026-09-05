/// 本机 HTTP 服务（shelf）—— 让桌面端把手机当对端：
/// /api/info、/api/message、/api/transfer/prepare（挂起等 UI 确认）、
/// /api/transfer/upload/{token}/{fid}、/api/transfer/cancel/{token}
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:path_provider/path_provider.dart';
import 'package:shelf/shelf.dart';
import 'package:shelf/shelf_io.dart' as shelf_io;
import 'package:shelf_router/shelf_router.dart';

import 'proto.dart';

class IncomingReq {
  final String reqId;
  final DeviceInfo peer;
  final List<FileMeta> files;
  final Completer<bool> decision;
  IncomingReq(this.reqId, this.peer, this.files, this.decision);
}

class RecvSession {
  final String token;
  final DeviceInfo peer;
  final List<FileMeta> files;
  final String saveDir;
  int completed = 0;
  RecvSession(this.token, this.peer, this.files, this.saveDir);
}

class AppServer {
  final DeviceInfo me;
  final void Function(ChatMsg msg, String peerId) onMessage;
  final void Function(IncomingReq req) onIncoming;
  final void Function(String peerId, String peerName, String fileName, int total,
      int transferred, bool done, String path) onProgress;
  HttpServer? _srv;
  final Map<String, RecvSession> _sessions = {};

  AppServer({
    required this.me,
    required this.onMessage,
    required this.onIncoming,
    required this.onProgress,
  });

  int get port => _srv?.port ?? 0;

  /// 从默认端口向后找可用端口（与桌面端行为一致）
  Future<int> start() async {
    var router = Router()
      ..get('/api/info', (Request r) => Response.ok(jsonEncode(me.toJson()),
          headers: {'Content-Type': 'application/json'}))
      ..post('/api/message', _handleMessage)
      ..post('/api/transfer/prepare', _handlePrepare)
      ..put('/api/transfer/upload/<token>/<fileId>', _handleUpload)
      ..post('/api/transfer/cancel/<token>', _handleCancel);
    final handler = const Pipeline().addMiddleware(logRequests()).addHandler(router.call);
    Object? lastErr;
    for (var p = defaultHttpPort; p < defaultHttpPort + 20; p++) {
      try {
        _srv = await shelf_io.serve(handler, InternetAddress.anyIPv4, p);
        return p;
      } catch (e) {
        lastErr = e;
      }
    }
    throw 'HTTP 服务启动失败：$lastErr';
  }

  Future<Response> _handleMessage(Request r) async {
    final body = jsonDecode(await r.readAsString()) as Map<String, dynamic>;
    final sender = DeviceInfo.fromJson(body['sender'] as Map<String, dynamic>);
    final text = (body['text'] as String?)?.trim() ?? '';
    if (text.isEmpty || sender.id == me.id) {
      return Response(400, body: '无效消息');
    }
    onMessage(
      ChatMsg(text: text, outgoing: false, atMs: DateTime.now().millisecondsSinceEpoch),
      sender.id,
    );
    return Response.ok('');
  }

  Future<Response> _handlePrepare(Request r) async {
    final body = jsonDecode(await r.readAsString()) as Map<String, dynamic>;
    final sender = DeviceInfo.fromJson(body['sender'] as Map<String, dynamic>);
    if (sender.id == me.id) return Response(400, body: '自己传自己？');
    final files = (body['files'] as List)
        .map((e) => FileMeta.fromJson(e as Map<String, dynamic>))
        .toList();
    if (files.isEmpty) return Response(400, body: '文件列表为空');

    // 挂起等 UI 确认（60s 超时，与桌面端一致）
    final req = IncomingReq(
        DateTime.now().microsecondsSinceEpoch.toString(), sender, files, Completer<bool>());
    onIncoming(req);
    bool ok;
    try {
      ok = await req.decision.future.timeout(const Duration(seconds: prepareTimeoutSecs));
    } on TimeoutException {
      return Response.forbidden('等待确认超时');
    }
    if (!ok) return Response.forbidden('对方拒绝了传输');

    final dir = await _saveDir();
    final token = DateTime.now().microsecondsSinceEpoch.toString();
    _sessions[token] = RecvSession(token, sender, files, dir);
    return Response.ok(jsonEncode({'token': token}),
        headers: {'Content-Type': 'application/json'});
  }

  Future<Response> _handleUpload(Request r, String token, String fileId) async {
    final sess = _sessions[token];
    if (sess == null) return Response(409, body: '会话不存在');
    final meta = sess.files.firstWhere((f) => f.id == fileId,
        orElse: () => FileMeta(id: fileId, name: fileId, relPath: fileId, size: 0));
    // 净化 rel_path（与桌面端一致：拒绝 .. / 绝对路径 / 盘符）
    final segs = meta.relPath
        .split('/')
        .where((s) => s.isNotEmpty && s != '.' && s != '..' && !s.contains(':'));
    final path = segs.isEmpty
        ? '${sess.saveDir}${Platform.pathSeparator}${meta.name}'
        : '${sess.saveDir}${Platform.pathSeparator}${segs.join(Platform.pathSeparator)}';
    final f = File(path);
    await f.parent.create(recursive: true);
    // v1：整读写入（shelf 便捷路径；超大文件的流式接收后续版本用 hijack）
    final bytes = await r.read().expand((c) => c).toList();
    await f.writeAsBytes(bytes, flush: true);
    sess.completed++;
    onProgress(sess.peer.id, sess.peer.name, meta.name, meta.size, bytes.length,
        true, path);
    if (sess.completed >= sess.files.length) {
      _sessions.remove(token);
    }
    return Response.ok('');
  }

  Future<Response> _handleCancel(Request r, String token) async {
    _sessions.remove(token);
    return Response.ok('');
  }

  Future<String> _saveDir() async {
    // Android：外部专项目录（无需权限，文件管理器可见）；iOS：文档目录
    final base = await getApplicationDocumentsDirectory();
    final dir = Directory('${base.parent.path}${Platform.pathSeparator}LocalTransfer');
    await dir.create(recursive: true);
    return dir.path;
  }

  Future<void> stop() async {
    await _srv?.close(force: true);
    _srv = null;
  }
}
