import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:localtransfer/proto.dart';

void main() {
  test('announce 编解码与桌面端 serde 格式一致', () {
    final me = DeviceInfo(
      id: 'abc-123',
      name: '手机-ABCD',
      plat: 'android',
      port: 17878,
    );
    final json = me.toAnnounce();
    // serde(tag="t", lowercase) + flatten：顶层 t + 展开的 DeviceInfo 字段
    expect(json['t'], 'announce');
    expect(json['id'], 'abc-123');
    expect(json['name'], '手机-ABCD');
    expect(json['plat'], 'android');
    expect(json['port'], 17878);
    expect(json['v'], protocolVersion);

    final (kind, info) = decodeAnnounce(jsonEncode(json));
    expect(kind, 'announce');
    expect(info!.id, 'abc-123');
    expect(info.name, '手机-ABCD');
    expect(info.port, 17878);
  });

  test('bye 解码', () {
    final (kind, info) = decodeAnnounce('{"t":"bye","id":"x"}');
    expect(kind, 'bye');
    expect(info, isNull);
  });

  test('非法包安全忽略', () {
    final (k1, i1) = decodeAnnounce('not json');
    expect(k1, '');
    expect(i1, isNull);
    final (k2, _) = decodeAnnounce('{"foo":1}');
    expect(k2, '');
  });

  test('FileMeta JSON 字段名与 Rust 端 snake_case 对应', () {
    final m = FileMeta(id: 'f1', name: 'a.txt', relPath: 'docs/a.txt', size: 12);
    final j = m.toJson();
    expect(j['rel_path'], 'docs/a.txt');
    final back = FileMeta.fromJson(j);
    expect(back.relPath, 'docs/a.txt');
    expect(back.size, 12);
  });

  test('fmtSize', () {
    expect(fmtSize(0), '0 B');
    expect(fmtSize(512), '512 B');
    expect(fmtSize(2048), '2.0 KB');
    expect(fmtSize(5 * 1024 * 1024), '5.0 MB');
  });
}
