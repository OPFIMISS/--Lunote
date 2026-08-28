import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'package:lunote_app/main.dart';
import 'package:lunote_app/src/state/app_state.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('设置与名称在退出核心后重进仍保持', (tester) async {
    final tmp = Directory.systemTemp.createTempSync('lunote_settings_');
    const port = 45884;

    Future<void> launch() async {
      await tester.pumpWidget(
        LunoteApp(
          dataDirOverride: tmp.path,
          nameOverride: '默认设备',
          tcpPortOverride: port,
        ),
      );
      for (var i = 0; i < 80 && !AppState.instance.coreReady; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }
      expect(AppState.instance.coreReady, true, reason: '核心未在 8 秒内启动');
    }

    await launch();
    final state = AppState.instance;
    expect(await state.setTheme('light'), isNull);
    expect(await state.setAutoTrust(false), isNull);
    expect(await state.renameDevice('持久设备名'), isNull);

    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(seconds: 3));

    await launch();
    expect(state.themeMode, 'light');
    expect(state.autoTrust, false);
    expect(state.deviceName, '持久设备名');
    expect(find.byType(MaterialApp), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(seconds: 3));
    try {
      tmp.deleteSync(recursive: true);
    } catch (_) {}
  });
}
