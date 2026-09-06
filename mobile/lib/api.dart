/// 桌面端 HTTP API 客户端：发文本、发起传输（prepare → 逐文件流式上传）。
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;

import 'discovery.dart';
import 'proto.dart';

class TransferApi {
  final DeviceInfo me;
  final http.Client _http = http.Client();

  TransferApi(this.me);

  /// 探测 /api/info（手动添加设备用）
  Future<DeviceInfo?> probe(String host, int port) async {
    try {
      final r = await _http
          .get(Uri.parse('http://$host:$port/api/info'))
          .timeout(const Duration(seconds: 3));
      if (r.statusCode != 200) return null;
      return DeviceInfo.fromJson(jsonDecode(r.body) as Map<String, dynamic>);
    } catch (_) {
      return null;
    }
  }

  /// 发文本
  Future<void> sendText(Peer peer, String text) async {
    final r = await _http.post(
      Uri.parse('${peer.httpBase}/api/message'),
      headers: {'Content-Type': 'application/json'},
      body: jsonEncode({
        'sender': me.toJson(),
        'text': text,
        'sent_at': DateTime.now().millisecondsSinceEpoch,
      }),
    );
    if (r.statusCode != 200) {
      throw '发送失败（${r.statusCode}）：${r.body}';
    }
  }

  /// 发一组文件：prepare（等桌面端确认，最多 60s）→ 逐文件流式上传。
  /// [onProgress] 回调 (fileIndex, transferred, total)。
  Future<void> sendFiles(
    Peer peer,
    List<(FileMeta, String)> files, // (meta, 本地路径)
    void Function(int idx, int transferred, int total) onProgress,
  ) async {
    // 1) prepare（桌面端确认前会挂起，给足超时）
    final pr = await _http
        .post(
          Uri.parse('${peer.httpBase}/api/transfer/prepare'),
          headers: {'Content-Type': 'application/json'},
          body: jsonEncode({
            'sender': me.toJson(),
            'files': files.map((f) => f.$1.toJson()).toList(),
          }),
        )
        .timeout(const Duration(seconds: prepareTimeoutSecs + 5));
    if (pr.statusCode == 403) {
      if (pr.body.contains('超时')) {
        throw '等待确认超时：请在 60 秒内在电脑端点「接收」';
      }
      throw '对方拒绝了传输';
    }
    if (pr.statusCode != 200) {
      throw '对方返回 ${pr.statusCode}：${pr.body}';
    }
    final token = (jsonDecode(pr.body) as Map<String, dynamic>)['token'] as String? ?? '';

    // 2) 逐文件上传（顺序，避免抢带宽；与桌面端行为一致）
    for (var i = 0; i < files.length; i++) {
      final (meta, path) = files[i];
      final f = File(path);
      final total = f.lengthSync();
      final req = http.StreamedRequest(
        'PUT',
        Uri.parse('${peer.httpBase}/api/transfer/upload/$token/${meta.id}'),
      );
      req.headers['Content-Type'] = 'application/octet-stream';
      req.contentLength = total;
      var transferred = 0;
      final raf = f.openSync();
      try {
        const chunk = 256 * 1024;
        var pos = 0;
        while (pos < total) {
          final n = (total - pos) < chunk ? (total - pos) : chunk;
          final buf = raf.readSync(n);
          transferred += buf.length;
          pos += buf.length;
          req.sink.add(buf);
          onProgress(i, transferred, total);
        }
      } finally {
        raf.closeSync();
      }
      await req.sink.close();
      final resp = await _http.send(req).timeout(const Duration(minutes: 10));
      if (resp.statusCode != 200) {
        // 中断后续文件：通知对方取消
        unawaited(_http.post(
            Uri.parse('${peer.httpBase}/api/transfer/cancel/$token')));
        throw '上传失败（${resp.statusCode}）';
      }
    }
  }

  /// 取消（尽力而为）
  Future<void> cancel(Peer peer, String token) async {
    try {
      await _http.post(Uri.parse('${peer.httpBase}/api/transfer/cancel/$token'));
    } catch (_) {}
  }
}
