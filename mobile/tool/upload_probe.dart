// 与 mobile/lib/api.dart 完全相同的上传实现（StreamedRequest，先填 sink 再 send），
// 直接对桌面端跑：验证上传代码本身是否有 bug。
// 运行：dart tool/upload_probe.dart <目标IP:端口> <文件路径>
import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;

void main(List<String> args) async {
  final base = 'http://${args[0]}';
  final path = args[1];
  final f = File(path);
  final total = f.lengthSync();
  final fileId = 'probe-f1';

  print('文件 ${total}B → $base');
  print('== prepare ==');
  final pr = await http.post(
    Uri.parse('$base/api/transfer/prepare'),
    headers: {'Content-Type': 'application/json'},
    body: jsonEncode({
      'sender': {
        'id': 'probe-dart-uploader',
        'name': 'Dart探针',
        'plat': 'windows',
        'port': 17878,
        'v': 1,
      },
      'files': [
        {'id': fileId, 'name': 'probe.bin', 'rel_path': 'probe.bin', 'size': total}
      ],
    }),
  ).timeout(const Duration(seconds: 65));
  print('prepare: ${pr.statusCode} ${pr.body}');
  if (pr.statusCode != 200) {
    print('FAIL at prepare');
    exit(1);
  }
  final token = jsonDecode(pr.body)['token'] as String;

  // ↓↓↓ 修复后的写法：先 send 再填 sink ↓↓↓
  print('== upload（先 send 再填 sink）==');
  final req = http.StreamedRequest(
    'PUT',
    Uri.parse('$base/api/transfer/upload/$token/$fileId'),
  );
  req.headers['Content-Type'] = 'application/octet-stream';
  req.contentLength = total;
  final client = http.Client();
  final respFuture = client.send(req);
  var transferred = 0;
  final raf = f.openSync();
  final sw = Stopwatch()..start();
  try {
    const chunk = 256 * 1024;
    var pos = 0;
    while (pos < total) {
      final n = (total - pos) < chunk ? (total - pos) : chunk;
      final buf = raf.readSync(n);
      transferred += buf.length;
      pos += buf.length;
      req.sink.add(buf);
    }
  } finally {
    raf.closeSync();
  }
  await req.sink.close();
  print('sink 填完 $transferred B，等响应...');
  final resp = await respFuture.timeout(const Duration(minutes: 2));
  final body = await resp.stream.bytesToString();
  print('upload: ${resp.statusCode} $body （${sw.elapsed}）');
  if (resp.statusCode != 200) {
    print('FAIL at upload');
    exit(1);
  }
  print('OK');
  exit(0);
}
