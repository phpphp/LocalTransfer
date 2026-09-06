import 'package:flutter_test/flutter_test.dart';
import 'package:localtransfer/discovery.dart';
import 'package:localtransfer/proto.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('手动添加的设备重启后恢复（持久化闭环）', () async {
    SharedPreferences.setMockInitialValues({});
    final me = DeviceInfo(id: 'me-1', name: '手机', plat: 'android', port: 17878);
    final d1 = Discovery(me);

    // 模拟手动添加成功后的持久化（绕过网络探测，直接入表+写盘）
    // 用 addManual 的内部路径不可行（会真的发 HTTP），这里测
    // _persistManual 的对外效果：通过再开一个 Discovery 能 restore 回来。
    // 直接构造 peers 再反射调用不可取——改为通过公共 API 验证：
    // 1) restoreManual 在空存储时无害
    await d1.restoreManual();
    expect(d1.peers.isEmpty, isTrue);

    // 2) 写入模拟数据后 restore 恢复
    SharedPreferences.setMockInitialValues({
      'manual_peers': ['desktop-uuid-1234|192.168.8.134|17878'],
    });
    final d2 = Discovery(me);
    await d2.restoreManual();
    expect(d2.peers.containsKey('desktop-uuid-1234'), isTrue);
    final p = d2.peers['desktop-uuid-1234']!;
    expect(p.manual, isTrue, reason: '恢复的设备必须保持 manual（不超时清理）');
    expect(p.addr.address, '192.168.8.134');
    expect(p.info.port, 17878);
    expect(p.online(deviceTimeoutSecs), isTrue, reason: 'manual 设备应视为在线');

    // 3) 手动设备不参与超时清理
    d2.debugPrune();
    expect(d2.peers.containsKey('desktop-uuid-1234'), isTrue);
  });
}
