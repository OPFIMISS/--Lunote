// 对话返回按钮集成测试：真实启动应用，验证
// 1) 宽屏（≥700）：对话页常驻返回按钮，点击回到对话列表；
// 2) 窄屏（<700）：push 模式返回按钮存在，点击后回到对话列表（不黑屏）。
// 运行：flutter test integration_test/back_button_test.dart -d windows

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'package:lunote_app/main.dart';
import 'package:lunote_app/src/core/models.dart';
import 'package:lunote_app/src/state/app_state.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  Future<void> waitReady(WidgetTester tester, AppState state) async {
    for (var i = 0; i < 100; i++) {
      await tester.pump(const Duration(milliseconds: 200));
      if (state.coreReady) return;
    }
    fail('核心未就绪');
  }

  void injectConversation(AppState state) {
    state.trusted['dev-test'] = TrustRecord(
      deviceId: 'dev-test',
      name: '测试对端',
      fingerprint: 'fp',
      trusted: true,
      firstSeenMs: 0,
      lastSeenMs: 0,
    );
    state.notifyListeners();
  }

  testWidgets('宽屏对话常驻返回按钮', (tester) async {
    tester.view.physicalSize = const Size(1280, 800);
    tester.view.devicePixelRatio = 1.0;
    final tmp = Directory.systemTemp.createTempSync('lunote_back_wide_');
    await tester.pumpWidget(LunoteApp(
      dataDirOverride: tmp.path,
      nameOverride: '返回测试',
      tcpPortOverride: 45882,
    ));
    final state = AppState.instance;
    await waitReady(tester, state);
    injectConversation(state);
    await tester.pump(const Duration(milliseconds: 300));

    // 切到对话页
    await tester.tap(find.text('对话').last);
    await tester.pump(const Duration(milliseconds: 500));
    // 点会话条目进入对话
    await tester.tap(find.text('测试对端').first);
    await tester.pump(const Duration(milliseconds: 500));

    // 宽屏内嵌：常驻返回按钮应存在
    final back = find.byIcon(Icons.arrow_back_rounded);
    expect(back, findsOneWidget, reason: '宽屏对话页缺少常驻返回按钮');

    // 点击返回 → 回到对话列表
    await tester.tap(back);
    await tester.pump(const Duration(milliseconds: 500));
    expect(find.text('对话'), findsWidgets, reason: '返回后未回到对话页');
    expect(back, findsNothing, reason: '返回后对话页应已关闭');

    await tester.pumpWidget(const SizedBox());
    tester.view.reset();
    try {
      tmp.deleteSync(recursive: true);
    } catch (_) {}
  });

  testWidgets('窄屏 push 返回不黑屏', (tester) async {
    tester.view.physicalSize = const Size(480, 900);
    tester.view.devicePixelRatio = 1.0;
    final tmp = Directory.systemTemp.createTempSync('lunote_back_narrow_');
    await tester.pumpWidget(LunoteApp(
      dataDirOverride: tmp.path,
      nameOverride: '返回测试2',
      tcpPortOverride: 45883,
    ));
    final state = AppState.instance;
    await waitReady(tester, state);
    injectConversation(state);
    await tester.pump(const Duration(milliseconds: 300));

    // 切到对话页（底部导航）
    await tester.tap(find.text('对话').last);
    await tester.pump(const Duration(milliseconds: 500));
    await tester.tap(find.text('测试对端').first);
    await tester.pump(const Duration(milliseconds: 500));

    final back = find.byIcon(Icons.arrow_back_rounded);
    expect(back, findsOneWidget, reason: '窄屏 push 对话页缺少返回按钮');

    // 点击返回 → 应回到对话列表（黑屏即此处断言失败/页面无内容）
    await tester.tap(back);
    await tester.pump(const Duration(milliseconds: 600));
    await tester.pump(const Duration(milliseconds: 600));
    expect(find.text('对话'), findsWidgets, reason: '窄屏返回后未回到对话列表（黑屏）');
    expect(find.text('测试对端'), findsOneWidget, reason: '窄屏返回后对话列表条目缺失');

    await tester.pumpWidget(const SizedBox());
    tester.view.reset();
    try {
      tmp.deleteSync(recursive: true);
    } catch (_) {}
  });
}
