import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';
import 'package:provider/provider.dart';
import 'package:tray_manager/tray_manager.dart';
import 'package:window_manager/window_manager.dart';

import 'src/core/window_ui.dart';
import 'src/state/app_state.dart';
import 'src/ui/lunote_theme.dart';
import 'src/ui/pages/shell_page.dart';

void main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // window_manager / tray_manager 没有 Android/iOS 实现：只在桌面平台初始化，
  // 否则 Android 上 ensureInitialized 抛 MissingPluginException 导致 runApp 不执行（白屏）。
  if (!Platform.isAndroid && !Platform.isIOS) {
    await windowManager.ensureInitialized();
  }
  runApp(const LunoteApp());
}

class LunoteApp extends StatefulWidget {
  const LunoteApp({
    super.key,
    this.dataDirOverride,
    this.nameOverride,
    this.tcpPortOverride,
  });

  /// 集成测试注入用；为空时走环境变量/默认路径
  final String? dataDirOverride;
  final String? nameOverride;

  /// 集成测试注入用：覆盖 TCP 端口，避免与正在运行的实例冲突
  final int? tcpPortOverride;

  @override
  State<LunoteApp> createState() => _LunoteAppState();
}

class _LunoteAppState extends State<LunoteApp> with WidgetsBindingObserver {
  String? _initError;
  bool _shutdownStarted = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    AppState.instance.addListener(_onStateChanged);
    _init();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.detached) {
      _shutdownCore();
    }
  }

  void _shutdownCore() {
    if (_shutdownStarted) return;
    _shutdownStarted = true;
    unawaited(AppState.instance.disposeCore());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _shutdownCore();
    super.dispose();
  }

  Future<void> _init() async {
    try {
      final state = AppState.instance;
      // 数据目录：应用支持目录/data；可用环境变量覆盖（测试/调试）
      var dataDir = 'data';
      try {
        final sup = await getApplicationSupportDirectory();
        dataDir = '${sup.path}${Platform.pathSeparator}data';
      } catch (e) {
        WindowUi.log('获取应用支持目录失败: $e，回退到相对目录 data/');
      }
      final envDir = Platform.environment['LUNOTE_DATA_DIR'];
      if (widget.dataDirOverride != null) {
        dataDir = widget.dataDirOverride!;
      } else if (envDir != null && envDir.isNotEmpty) {
        dataDir = envDir;
      }
      final envName = Platform.environment['LUNOTE_NAME'];
      final envBridge = Platform.environment['LUNOTE_BRIDGE_PATH'];
      final detectedName = await _detectDeviceName();
      await state.init(
        dataDir: dataDir,
        name:
            widget.nameOverride ??
            ((envName != null && envName.isNotEmpty)
                ? envName
                : (detectedName ?? '我的设备')),
        tcpPort: widget.tcpPortOverride ?? 45455,
        bridgeOverride: envBridge,
      );
      // 新安装或仍使用旧默认名时，使用平台真实设备型号作为可识别名称。
      // 用户已经自定义过的名称绝不覆盖。
      if (detectedName != null &&
          (state.deviceName.isEmpty ||
              state.deviceName == '我的设备' ||
              state.deviceName == 'Android 设备')) {
        await state.renameDevice(detectedName);
      }
      if (mounted) setState(() {});
      if (Platform.isAndroid) {
        try {
          await const MethodChannel('com.lunote.lunote_app/platform')
              .invokeMethod('requestNotificationPermission');
        } catch (_) {
          // 通知权限不是核心功能，拒绝时继续正常运行。
        }
      }
      if (!Platform.isAndroid && !Platform.isIOS) {
        // 窗口/托盘设置失败只记录日志并降级，不阻塞应用使用
        try {
          await _setupDesktopWindow();
        } catch (e, st) {
          WindowUi.log('窗口设置失败（降级继续）: $e\n$st');
        }
        await _setupTray();
      }
    } catch (e, st) {
      WindowUi.log('初始化失败: $e\n$st');
      if (mounted) setState(() => _initError = '$e');
    }
  }

  Future<String?> _detectDeviceName() async {
    if (Platform.isAndroid) {
      try {
        return await const MethodChannel('com.lunote.lunote_app/platform')
            .invokeMethod<String>('getDeviceModel');
      } catch (_) {
        return 'Android 设备';
      }
    }
    if (Platform.isWindows) {
      final name = Platform.environment['COMPUTERNAME'];
      return name == null || name.isEmpty ? Platform.localHostname : name;
    }
    return Platform.localHostname;
  }

  Future<void> _setupDesktopWindow() async {
    // 关键：main() 里首次 ensureInitialized 时窗口可能尚未创建，
    // native 侧 native_window 句柄为 NULL 会导致所有窗口操作静默失败。
    // 此处窗口已就绪，再次调用会重新获取有效句柄（该方法非幂等，每次都会刷新）。
    await windowManager.ensureInitialized();
    await windowManager.setTitleBarStyle(
      TitleBarStyle.hidden,
      windowButtonVisibility: false,
    );
    await windowManager.setSize(const Size(1180, 760));
    await windowManager.setMinimumSize(const Size(940, 620));
    try {
      await windowManager.center();
    } catch (e) {
      // 窗口尚未显示时 getBounds 可能返回 null；居中失败不致命
      WindowUi.log('窗口居中失败（忽略）: $e');
    }
    // 不拦截系统关闭：Alt+F4 / 任务栏关闭 = 正常退出
    await windowManager.show();
  }

  Future<void> _setupTray() async {
    try {
      final dir = await getTemporaryDirectory();
      final isWindows = Platform.isWindows;
      final iconPath =
          '${dir.path}${Platform.pathSeparator}lunote_tray.${isWindows ? 'ico' : 'png'}';
      await _writeMoonIcon(iconPath, isWindows);
      await trayManager.setIcon(iconPath);
      await trayManager.setToolTip('月笺 Lunote');
      await trayManager.setContextMenu(
        Menu(
          items: [
            MenuItem(key: 'show', label: '显示主窗口'),
            MenuItem(key: 'quit', label: '退出'),
          ],
        ),
      );
      trayManager.addListener(_LunoteTrayListener());
      WindowUi.trayOk = true;
    } catch (e) {
      // 托盘不可用（如某些 Linux 环境）时静默降级：关闭按钮改为退出程序
      WindowUi.log('托盘初始化失败（关闭按钮将改为退出）: $e');
      WindowUi.trayOk = false;
    }
  }

  /// 运行时绘制“月牙托住信笺”图标（PNG；Windows 额外封装为 ICO 容器）
  Future<void> _writeMoonIcon(String path, bool ico) async {
    final recorder = ui.PictureRecorder();
    final canvas = Canvas(recorder);
    const s = 64.0;
    final bg = Paint()..color = const Color(0xFF163A4A);
    canvas.drawRRect(
      RRect.fromRectAndRadius(
        const Rect.fromLTWH(0, 0, s, s),
        const Radius.circular(14),
      ),
      bg,
    );
    final gold = Paint()..color = const Color(0xFFF0B94D);
    canvas.drawCircle(const Offset(27, 33), 19, gold);
    final cut = Paint()..color = const Color(0xFF163A4A);
    canvas.drawCircle(const Offset(35, 25), 15.5, cut);
    final paper = Path()
      ..moveTo(28, 18)
      ..lineTo(43, 18)
      ..lineTo(51, 26)
      ..lineTo(51, 47)
      ..quadraticBezierTo(51, 50, 48, 50)
      ..lineTo(28, 50)
      ..quadraticBezierTo(25, 50, 25, 47)
      ..lineTo(25, 21)
      ..quadraticBezierTo(25, 18, 28, 18)
      ..close();
    canvas.drawPath(paper, Paint()..color = Colors.white);
    final fold = Path()
      ..moveTo(43, 18)
      ..lineTo(43, 26)
      ..lineTo(51, 26)
      ..close();
    canvas.drawPath(fold, Paint()..color = const Color(0xFFD8EEF0));
    final ink = Paint()
      ..color = const Color(0xFF3E7080)
      ..strokeWidth = 2.2
      ..strokeCap = StrokeCap.round;
    canvas.drawLine(const Offset(31, 34), const Offset(45, 34), ink);
    canvas.drawLine(const Offset(31, 40), const Offset(42, 40), ink);
    final pic = recorder.endRecording();
    final img = await pic.toImage(64, 64);
    final bytes = await img.toByteData(format: ui.ImageByteFormat.png);
    final png = bytes!.buffer.asUint8List();
    if (!ico) {
      File(path).writeAsBytesSync(png);
      return;
    }
    File(path).writeAsBytesSync(_wrapIco(png));
  }

  /// 最小 ICO 容器（Vista+ 支持 PNG 载荷）
  List<int> _wrapIco(List<int> png) {
    const count = 1;
    final header = <int>[0, 0, 1, 0, count, 0];
    final dir = <int>[
      64,
      64,
      0,
      0,
      0,
      0,
      1,
      0,
      png.length & 0xFF,
      (png.length >> 8) & 0xFF,
      (png.length >> 16) & 0xFF,
      (png.length >> 24) & 0xFF,
      22,
      0,
      0,
      0,
    ];
    return [...header, ...dir, ...png];
  }

  @override
  Widget build(BuildContext context) {
    return ChangeNotifierProvider.value(
      value: AppState.instance,
      child: _AppMaterial(initError: _initError),
    );
  }
}

/// Provider 内的 MaterialApp：跟随主题设置切换深/浅
class _AppMaterial extends StatelessWidget {
  const _AppMaterial({required this.initError});

  final String? initError;

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final mode = switch (state.themeMode) {
      'light' => ThemeMode.light,
      'system' => ThemeMode.system,
      'glass' => ThemeMode.light,
      _ => ThemeMode.dark,
    };
    return MaterialApp(
      title: '月笺 Lunote',
      debugShowCheckedModeBanner: false,
      theme: state.themeMode == 'glass'
          ? LunoteTheme.glass()
          : LunoteTheme.light(),
      darkTheme: LunoteTheme.dark(), // ThemeMode.dark 时使用
      themeMode: mode,
      builder: (context, child) {
        final cc = LunoteColors.of(context);
        if (!cc.isGlass) return child ?? const SizedBox.shrink();
        return DecoratedBox(
          decoration: const BoxDecoration(
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [Color(0xFFF7D9E5), Color(0xFFD9EFF2), Color(0xFFF6E7BC)],
            ),
          ),
          child: ClipRect(
            child: BackdropFilter(
              filter: ui.ImageFilter.blur(sigmaX: 16, sigmaY: 16),
              child: child ?? const SizedBox.shrink(),
            ),
          ),
        );
      },
      home: initError != null ? _ErrorView(error: initError!) : const _LockGate(),
    );
  }
}

class _LockGate extends StatefulWidget {
  const _LockGate();
  @override State<_LockGate> createState() => _LockGateState();
}

class _LockGateState extends State<_LockGate> with WidgetsBindingObserver {
  bool _locked = false;
  bool _checking = false;
  int _failedAttempts = 0;
  DateTime? _lockedUntil;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    WidgetsBinding.instance.addPostFrameCallback((_) => _sync());
  }

  @override
  void dispose() { AppState.instance.removeListener(_onStateChanged); WidgetsBinding.instance.removeObserver(this); super.dispose(); }

  void _onStateChanged() {
    if (!mounted) return;
    final enabled = AppState.instance.pinEnabled;
    if (enabled && !_locked) setState(() => _locked = true);
    if (!enabled && _locked) setState(() => _locked = false);
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.paused && AppState.instance.pinEnabled) {
      setState(() => _locked = true);
    }
  }

  Future<void> _sync() async {
    if (AppState.instance.pinEnabled && mounted) setState(() => _locked = true);
  }

  Future<void> _unlock() async {
    final until = _lockedUntil;
    if (until != null && DateTime.now().isBefore(until)) return;
    final ctrl = TextEditingController();
    final pin = await showDialog<String>(
      context: context,
      barrierDismissible: false,
      builder: (ctx) => AlertDialog(
        title: const Text('应用已锁定'),
        content: TextField(controller: ctrl, autofocus: true, obscureText: true, keyboardType: TextInputType.number, decoration: const InputDecoration(labelText: '输入 PIN')),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.pop(ctx, ctrl.text), child: const Text('解锁')),
        ],
      ),
    );
    if (pin == null) return;
    setState(() => _checking = true);
    final ok = await AppState.instance.verifyPin(pin);
    if (mounted) {
      setState(() {
        _checking = false;
        if (ok) { _locked = false; _failedAttempts = 0; _lockedUntil = null; }
        else {
          _failedAttempts++;
          if (_failedAttempts >= 5) { _lockedUntil = DateTime.now().add(const Duration(seconds: 30)); _failedAttempts = 0; }
        }
      });
      if (!ok) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(_lockedUntil != null ? '尝试次数过多，请 30 秒后再试' : 'PIN 不正确')));
    }
  }

  @override
  Widget build(BuildContext context) {
    if (!_locked) return const _Home();
    return Scaffold(body: Center(child: _checking ? const CircularProgressIndicator() : FilledButton.icon(onPressed: _unlock, icon: const Icon(Icons.lock_open_rounded), label: const Text('解锁月笺'))));
  }
}

class _Home extends StatelessWidget {
  const _Home();

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    final isDesktop = !Platform.isAndroid && !Platform.isIOS;
    return Scaffold(
      body: Column(
        children: [
          if (isDesktop) const _TitleBar(),
          Expanded(
            child: Consumer<AppState>(
              builder: (context, state, _) {
                if (!state.coreReady) {
                  return Center(
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        SizedBox(
                          width: 30,
                          height: 30,
                          child: CircularProgressIndicator(
                            strokeWidth: 3,
                            color: cc.gold,
                          ),
                        ),
                        SizedBox(height: 14),
                        Text('月笺正在启动…', style: TextStyle(color: cc.moonDim)),
                      ],
                    ),
                  );
                }
                return const ShellPage();
              },
            ),
          ),
        ],
      ),
    );
  }
}

class _LunoteTrayListener extends TrayListener {
  @override
  void onTrayIconMouseDown() {
    windowManager.show();
  }

  @override
  void onTrayIconRightMouseDown() {
    // tray_manager 的 Windows 实现不会自动弹菜单，必须手动调用
    trayManager.popUpContextMenu();
  }

  @override
  void onTrayMenuItemClick(MenuItem menuItem) {
    switch (menuItem.key) {
      case 'show':
        windowManager.show();
      case 'quit':
        trayManager.destroy();
        windowManager.destroy();
        exit(0);
    }
  }
}

/// 自定义标题栏：拖拽区（DragToMoveArea）与窗口按钮（官方 WindowCaptionButton）分离，
/// 避免手势竞争导致按钮失效。
class _TitleBar extends StatelessWidget {
  const _TitleBar();

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    return Container(
      height: 40,
      decoration: BoxDecoration(
        color: cc.nightRaised,
        border: Border(bottom: BorderSide(color: cc.nightSoft, width: 1)),
      ),
      child: Row(
        children: [
          // 拖拽区：覆盖标题与空白区域（按钮区之外）
          Expanded(
            child: DragToMoveArea(
              child: SizedBox(
                height: 40,
                child: Row(
                  children: [
                    const SizedBox(width: 14),
                    Text(
                      '月笺 Lunote',
                      style: TextStyle(
                        fontSize: 11.5,
                        color: cc.moonDim,
                        letterSpacing: 1,
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
          // 官方窗口按钮（自带悬停/按下态，且不参与拖拽手势）
          WindowCaptionButton.minimize(
            key: const Key('btn_minimize'),
            onPressed: () => WindowUi.op('minimize', windowManager.minimize),
          ),
          WindowCaptionButton.maximize(
            key: const Key('btn_maximize'),
            onPressed: () => WindowUi.op('toggleMaximize', () async {
              final maximized = await windowManager.isMaximized();
              if (maximized) {
                await windowManager.unmaximize();
              } else {
                await windowManager.maximize();
              }
            }),
          ),
          WindowCaptionButton.close(
            key: const Key('btn_close'),
            onPressed: () {
              // 关闭 = 真正退出应用（不隐藏到托盘）
              WindowUi.log('窗口操作已执行: close（退出应用）');
              windowManager
                  .destroy()
                  .then((_) => exit(0))
                  .catchError((_) => exit(0));
            },
          ),
        ],
      ),
    );
  }
}

class _ErrorView extends StatelessWidget {
  const _ErrorView({required this.error});

  final String error;

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    return Scaffold(
      body: Center(
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.error_outline_rounded, size: 44, color: cc.warn),
              const SizedBox(height: 14),
              Text(
                '核心启动失败',
                style: TextStyle(
                  fontSize: 17,
                  fontWeight: FontWeight.w700,
                  color: cc.moon,
                ),
              ),
              const SizedBox(height: 10),
              Text(
                error,
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 12.5, color: cc.moonDim),
              ),
              const SizedBox(height: 18),
              OutlinedButton(
                onPressed: () => SystemNavigator.pop(),
                child: const Text('退出'),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
