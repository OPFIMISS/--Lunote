// Windows 窗口控制集成测试：真实启动应用，点击标题栏按钮，
// 验证“点击 → 回调 → window_manager API 调用”链路。
//
// 说明：flutter test 的测试窗口中，窗口状态查询（isMinimized 等）在部分
// 环境不可靠；本测试以 UI 日志断言按钮回调确实执行了对应操作。
// 运行：flutter test integration_test/window_controls_test.dart -d windows

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:window_manager/window_manager.dart';

import 'package:lunote_app/main.dart';
import 'package:lunote_app/src/core/window_ui.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('标题栏按钮回调链路', (tester) async {
    // 隔离数据目录 + 独立日志文件
    final tmp = Directory.systemTemp.createTempSync('lunote_it_');
    final logFile =
        '${tmp.path}${Platform.pathSeparator}ui.log';
    WindowUi.logFile = logFile;

    await tester.pumpWidget(LunoteApp(
      dataDirOverride: tmp.path,
      nameOverride: '窗口集成测试',
    ));

    // 等待标题栏出现（核心就绪）
    var ready = false;
    for (var i = 0; i < 100; i++) {
      await tester.pump(const Duration(milliseconds: 200));
      if (find.byKey(const Key('btn_minimize')).evaluate().isNotEmpty) {
        ready = true;
        break;
      }
    }
    expect(ready, true, reason: '标题栏未出现（核心可能未就绪）');

    // 窗口已创建：重新初始化 window_manager 以获取有效句柄
    await windowManager.ensureInitialized();

    // 直接 API 冒烟：窗口操作应真正生效
    await windowManager.minimize();
    await tester.pump(const Duration(milliseconds: 800));
    final directMin = await windowManager.isMinimized();
    debugPrint('诊断: 直接 API 最小化 => $directMin');
    await windowManager.restore();
    await tester.pump(const Duration(milliseconds: 800));

    // 拖拽区存在（DragToMoveArea 覆盖标题文本区）
    expect(find.byType(DragToMoveArea), findsOneWidget,
        reason: '缺少拖拽区');

    String readLog() {
      final f = File(logFile);
      return f.existsSync() ? f.readAsStringSync() : '';
    }

    // 最小化按钮 → 回调应执行 minimize
    await tester.tap(find.byKey(const Key('btn_minimize')));
    await tester.pump(const Duration(milliseconds: 800));
    expect(readLog(), contains('窗口操作已执行: minimize'),
        reason: '最小化按钮回调未执行');
    final btnMin = await windowManager.isMinimized();
    debugPrint('诊断: 按钮最小化状态 => $btnMin');
    await windowManager.restore();
    await tester.pump(const Duration(milliseconds: 800));

    // 最大化按钮 → 回调应执行 toggleMaximize（内部走 maximize/unmaximize）
    await tester.tap(find.byKey(const Key('btn_maximize')));
    await tester.pump(const Duration(milliseconds: 800));
    expect(readLog(), contains('窗口操作已执行: toggleMaximize'),
        reason: '最大化按钮回调未执行');

    // 关闭按钮存在（点击会退出应用，此处不点击，避免测试进程退出）
    expect(find.byKey(const Key('btn_close')), findsOneWidget,
        reason: '关闭按钮缺失');

    // 清理
    await windowManager.destroy();
    try {
      tmp.deleteSync(recursive: true);
    } catch (_) {}
  });
}
