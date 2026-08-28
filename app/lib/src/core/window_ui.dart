import 'dart:io';

/// 桌面窗口/UI 诊断日志（写 %TEMP%\lunote_ui.log，便于远程定位界面问题）。
class WindowUi {
  static bool trayOk = false;

  /// 日志文件路径（集成测试可覆盖）
  static String logFile =
      '${Directory.systemTemp.path}${Platform.pathSeparator}lunote_ui.log';

  /// 窗口操作统一捕获异常并写日志（便于远程诊断）
  static Future<void> op(String name, Future<void> Function() fn) async {
    try {
      await fn();
      log('窗口操作已执行: $name');
    } catch (e, st) {
      log('窗口操作失败 $name: $e\n$st');
    }
  }

  static void log(String msg) {
    try {
      final f = File(logFile);
      f.writeAsStringSync('${DateTime.now()} $msg\n', mode: FileMode.append);
    } catch (_) {}
  }
}
