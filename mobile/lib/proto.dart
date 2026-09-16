/// 传输协议模型 —— 与 crates/core/src/proto.rs 严格对应。
/// 线上格式（JSON）：
///  - 发现（UDP 多播 239.192.71.82:17878）：
///      announce: {"t":"announce","id":..,"name":..,"plat":..,"port":..,"v":1}
///      bye:      {"t":"bye","id":"..."}
///  - HTTP：/api/info、/api/message、/api/transfer/prepare → {"token":..}、
///          PUT /api/transfer/upload/{token}/{fileId}（原始流）、/api/transfer/cancel/{token}
library;

import 'dart:convert';

const int protocolVersion = 1;
const String discoveryGroup = '239.192.71.82';
const int discoveryPort = 17878;
const int defaultHttpPort = 17878;
const int deviceTimeoutSecs = 15;
const int prepareTimeoutSecs = 300;

class DeviceInfo {
  final String id;
  String name; // 改名立即生效（下一条 announce 带新名）
  final String plat; // windows / mac / linux / android / ios
  int port; // 服务启动后回填（可变）
  final int v;

  DeviceInfo({
    required this.id,
    required this.name,
    required this.plat,
    required this.port,
    this.v = protocolVersion,
  });

  Map<String, dynamic> toJson() =>
      {'id': id, 'name': name, 'plat': plat, 'port': port, 'v': v};

  factory DeviceInfo.fromJson(Map<String, dynamic> j) => DeviceInfo(
        id: j['id'] as String,
        name: j['name'] as String,
        plat: (j['plat'] as String?) ?? 'unknown',
        port: (j['port'] as num).toInt(),
        v: ((j['v'] as num?) ?? 0).toInt(),
      );

  Map<String, dynamic> toAnnounce() =>
      {'t': 'announce', ...toJson()};
}

class FileMeta {
  final String id;
  final String name;
  final String relPath;
  final int size;

  FileMeta({
    required this.id,
    required this.name,
    required this.relPath,
    required this.size,
  });

  Map<String, dynamic> toJson() =>
      {'id': id, 'name': name, 'rel_path': relPath, 'size': size};

  factory FileMeta.fromJson(Map<String, dynamic> j) => FileMeta(
        id: j['id'] as String,
        name: j['name'] as String,
        relPath: j['rel_path'] as String,
        size: (j['size'] as num).toInt(),
      );
}

/// 发现包编解码
(String, DeviceInfo?) decodeAnnounce(String raw) {
  final Map<String, dynamic> j;
  try {
    j = jsonDecode(raw) as Map<String, dynamic>;
  } catch (_) {
    return ('', null);
  }
  final t = (j['t'] as String?) ?? '';
  if (t == 'announce') {
    return ('announce', DeviceInfo.fromJson(j));
  } else if (t == 'bye') {
    return ('bye', null);
  }
  return ('', null);
}

/// 接收批次里的一个文件（打开用）
class FileEntry {
  final String name; // 显示名（rel 尾段）
  final String relPath;
  final int size;
  final String? uri; // MediaStore 内容 URI
  final String? publicPath; // 真实路径（回退用）
  FileEntry(this.name, this.relPath, this.size, this.uri, this.publicPath);
}

class ChatMsg {
  final String text; // 文本内容
  final String fileName; // 文件消息的名字（空 = 文本）
  final int fileSize;
  final bool outgoing;
  final int atMs;
  final String? path; // 打开目标（uri:…/path:…/folder:… 约定，见 main.dart）
  final String? location; // 保存位置（人类可读，卡片展示）
  final List<FileEntry>? files; // 批次文件清单（应用内浏览/逐个打开）
  ChatMsg({
    this.text = '',
    this.fileName = '',
    this.fileSize = 0,
    required this.outgoing,
    required this.atMs,
    this.path,
    this.location,
    this.files,
  });
  bool get isFile => fileName.isNotEmpty;
}

/// 一次进行中传输的进度（发送或接收）
class TransferProgress {
  final int transferred;
  final int total;
  final bool done;
  const TransferProgress(this.transferred, this.total, this.done);
  double get frac =>
      total > 0 ? (transferred / total).clamp(0.0, 1.0) : (done ? 1.0 : 0.0);
}

String fmtSize(int bytes) {
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  var v = bytes.toDouble();
  var i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return i == 0 ? '$bytes B' : '${v.toStringAsFixed(1)} ${u[i]}';
}
