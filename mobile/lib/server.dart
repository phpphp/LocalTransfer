/// 本机 HTTP 服务（shelf）—— 让桌面端把手机当对端：
/// /api/info、/api/message、/api/transfer/prepare（挂起等 UI 确认）、
/// /api/transfer/upload/{token}/{fid}、/api/transfer/cancel/{token}
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';
import 'package:shelf/shelf.dart';
import 'package:shelf/shelf_io.dart' as shelf_io;
import 'package:shelf_router/shelf_router.dart';

import 'proto.dart';

/// 接收决策：accept=false 拒绝；accept=true 时 treeUri 非空表示
/// 用户"存到…"选定的 SAF 目录（本批文件转存到该目录而非默认下载目录）
class IncomingDecision {
  final bool accept;
  final String? treeUri;
  IncomingDecision(this.accept, this.treeUri);
}

class IncomingReq {
  final String reqId;
  final DeviceInfo peer;
  final List<FileMeta> files;
  final Completer<IncomingDecision> decision;
  IncomingReq(this.reqId, this.peer, this.files, this.decision);
}

/// 一个已完成文件的信息（转存公共下载目录后）
class DoneFile {
  final String relPath;
  final int size;
  final String? publicPath; // Download/LocalTransfer/... 下的真实路径（失败为 null）
  final String? uri; // MediaStore content URI（打开/分享用，失败为 null）
  DoneFile(this.relPath, this.size, this.publicPath, this.uri);
}

/// 一次接收会话的实时进度（UI 展示用）
class RecvProgress {
  final String peerId;
  final String label; // 文件夹名或文件名
  final int fileIdx;
  final int fileCount;
  int transferred;
  int total;
  RecvProgress(this.peerId, this.label, this.fileIdx, this.fileCount,
      this.transferred, this.total);
}

class RecvSession {
  final String token;
  final DeviceInfo peer;
  final List<FileMeta> files;
  final String saveDir;
  /// 用户"存到…"选定的 SAF 目录（null=默认转存 Download/LocalTransfer）
  final String? treeUri;
  int completed = 0;
  final List<DoneFile> doneFiles = [];
  RecvSession(this.token, this.peer, this.files, this.saveDir, this.treeUri);
}

class AppServer {
  final DeviceInfo me;
  final void Function(ChatMsg msg, String peerId) onMessage;
  final void Function(IncomingReq req) onIncoming;
  /// 整批传输完成（一次 prepare 会话的所有文件）→ 合并为一张卡片
  final void Function(String peerId, DeviceInfo peer, List<DoneFile> files,
      String saveDir) onBatchDone;
  /// 接收进度（每读一块回调一次；UI 节流显示）
  final void Function(String token, RecvProgress p) onRecvProgress;
  /// 会话结束（完成/取消）→ 清理进度
  final void Function(String token) onRecvEnd;
  HttpServer? _srv;
  final Map<String, RecvSession> _sessions = {};

  AppServer({
    required this.me,
    required this.onMessage,
    required this.onIncoming,
    required this.onBatchDone,
    required this.onRecvProgress,
    required this.onRecvEnd,
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

    // 挂起等 UI 确认（5 分钟超时，与桌面端一致——对方可用"存到…"从容选目录）
    final req = IncomingReq(DateTime.now().microsecondsSinceEpoch.toString(), sender,
        files, Completer<IncomingDecision>());
    onIncoming(req);
    IncomingDecision d;
    try {
      d = await req.decision.future.timeout(const Duration(seconds: prepareTimeoutSecs));
    } on TimeoutException {
      return Response.forbidden('等待确认超时');
    }
    if (!d.accept) return Response.forbidden('对方拒绝了传输');

    final dir = await _saveDir();
    final token = DateTime.now().microsecondsSinceEpoch.toString();
    _sessions[token] = RecvSession(token, sender, files, dir, d.treeUri);
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
    final relClean = segs.join('/');
    final path = segs.isEmpty
        ? '${sess.saveDir}${Platform.pathSeparator}${meta.name}'
        : '${sess.saveDir}${Platform.pathSeparator}${segs.join(Platform.pathSeparator)}';
    final f = File(path);
    await f.parent.create(recursive: true);
    // 流式写入（不再整读进内存）+ 每块回调进度
    final sink = f.openWrite();
    var written = 0;
    final label = relClean.contains('/')
        ? relClean.substring(0, relClean.indexOf('/'))
        : relClean;
    await for (final chunk in r.read()) {
      sink.add(chunk);
      written += chunk.length;
      onRecvProgress(
          token,
          RecvProgress(sess.peer.id, label, sess.completed, sess.files.length,
              written, meta.size));
    }
    await sink.flush();
    await sink.close();
    sess.completed++;
    // 转存公共下载目录（Download/LocalTransfer/...），文件管理器可见；
    // "存到…"选过目录则转存到该 SAF 目录
    final pub = await _publishToDownloads(path, relClean, sess.treeUri);
    final (publicPath, uri) = pub;
    sess.doneFiles.add(DoneFile(
      relClean.isEmpty ? meta.name : relClean,
      written,
      publicPath,
      uri,
    ));
    if (sess.completed >= sess.files.length) {
      _sessions.remove(token);
      onRecvEnd(token);
      // 整批完成 → 合并为一张卡片（文件夹名 / "N 个文件"）
      onBatchDone(sess.peer.id, sess.peer, sess.doneFiles, sess.saveDir);
    }
    return Response.ok('');
  }

  /// 复制进公共下载目录（Android 平台通道 MediaStore）；
  /// [treeUri] 非空时转存到用户选定的 SAF 目录。
  /// 返回 (真实路径?, contentURI?)。
  Future<(String?, String?)> _publishToDownloads(
      String srcPath, String relPath, String? treeUri) async {
    try {
      final r = await const MethodChannel('localtransfer/downloads')
          .invokeMethod<String?>('save', {
        'path': srcPath,
        'rel': relPath,
        if (treeUri != null) 'treeUri': treeUri,
      });
      if (r == null || r.isEmpty) return (null, null);
      // Kotlin 返回 "path|uri"（path 可能为 "null"）
      final parts = r.split('|');
      final p = parts.isNotEmpty && parts[0] != 'null' ? parts[0] : null;
      final u = parts.length > 1 && parts[1] != 'null' ? parts[1] : null;
      return (p, u);
    } on PlatformException {
      return (null, null); // 非 Android / 失败：文件留在应用目录，仍可打开
    } on MissingPluginException {
      return (null, null);
    }
  }

  Future<Response> _handleCancel(Request r, String token) async {
    _sessions.remove(token);
    onRecvEnd(token);
    return Response.ok('');
  }

  Future<String> _saveDir() async {
    // 临时落盘目录（随后转存公共下载目录）；
    // 优先应用外部目录（无需权限、容量大）
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
