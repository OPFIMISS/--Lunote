// 设备发现集成测试：真实启动应用（独立数据目录/端口），
// 验证“核心发现 → Dart 状态 → 设备页渲染”整条链路。
// 前提：局域网内至少有一台在线设备（如模拟器或正在运行的 PC 实例）。
// 运行：flutter test integration_test/device_discovery_test.dart -d windows

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'package:lunote_app/main.dart';
import 'package:lunote_app/src/state/app_state.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('设备页显示真实发现的设备', (tester) async {
    final tmp = Directory.systemTemp.createTempSync('lunote_disc_');
    await tester.pumpWidget(LunoteApp(
      dataDirOverride: tmp.path,
      nameOverride: '发现集成测试',
      tcpPortOverride: 45881, // 避开正在运行的实例（45455/45672）
    ));

    final state = AppState.instance;
    var ready = false;
    for (var i = 0; i < 120; i++) {
      await tester.pump(const Duration(milliseconds: 250));
      if (state.coreReady && state.peers.values.any((p) => p.online)) {
        ready = true;
        break;
      }
    }
    debugPrint(
      '诊断: peers=${state.peers.values.map((p) => '${p.name}@${p.ip}:${p.tcpPort}(online=${p.online})').join(', ')}',
    );
    expect(ready, true, reason: '30 秒内未发现任何在线设备（核心发现链路异常）');

    // 切到设备页（窄屏底部导航 / 宽屏侧栏都包含“设备”入口）
    final deviceTab = find.text('设备').last;
    await tester.tap(deviceTab);
    await tester.pump(const Duration(milliseconds: 600));

    // 设备页应渲染出设备卡片（含在线状态与 IP）
    expect(find.textContaining('在线'), findsWidgets,
        reason: '设备页标题未显示在线数');
    expect(
      find.byWidgetPredicate((w) =>
          w is Text && w.data != null && w.data!.contains(RegExp(r'\d+\.\d+\.\d+\.\d+'))),
      findsWidgets,
      reason: '设备页未渲染任何设备 IP',
    );

    // 清理
    await tester.pumpWidget(const SizedBox());
    try {
      tmp.deleteSync(recursive: true);
    } catch (_) {}
  });
}
