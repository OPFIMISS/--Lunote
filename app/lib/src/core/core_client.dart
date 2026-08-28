import 'dart:async';
import 'dart:convert';
import 'dart:isolate';

import 'core_bridge.dart';

/// 核心客户端：在专用 isolate 中执行 FFI 命令并轮询事件，
/// 主 isolate 只消费事件流与命令结果，不阻塞渲染。
class CoreClient {
  CoreClient._();

  static final CoreClient instance = CoreClient._();

  int? _handle;
  SendPort? _workerPort;
  Isolate? _workerIsolate;
  ReceivePort? _mainPort;
  StreamSubscription<dynamic>? _mainPortSubscription;
  Completer<SendPort>? _workerReady;
  Completer<void>? _workerStopped;
  final _eventController = StreamController<Map<String, dynamic>>.broadcast();
  final Map<int, Completer<Map<String, dynamic>>> _pending = {};
  int _reqId = 0;

  Stream<Map<String, dynamic>> get events => _eventController.stream;

  bool get isRunning => _handle != null;

  /// 启动核心（dataDir/name/discoveryPort/tcpPort）
  Future<void> start({
    required String dataDir,
    required String name,
    int discoveryPort = 45454,
    int tcpPort = 45455,
    String? bridgeOverride,
  }) async {
    if (_handle != null) {
      return;
    }
    CoreNative.instance.load(overridePath: bridgeOverride);
    final config = jsonEncode({
      'data_dir': dataDir,
      'name': name,
      'discovery_port': discoveryPort,
      'tcp_port': tcpPort,
    });
    final handle = CoreNative.instance.create(config);
    if (handle <= 0) {
      throw StateError('核心启动失败（句柄 $handle）');
    }
    final mainPort = ReceivePort();
    _mainPort = mainPort;
    _workerReady = Completer<SendPort>();
    _mainPortSubscription = mainPort.listen(_onWorkerMessage);
    try {
      _workerIsolate = await Isolate.spawn(_workerMain, [
        mainPort.sendPort,
        handle,
        bridgeOverride,
      ]);
      _workerPort = await _workerReady!.future.timeout(
        const Duration(seconds: 10),
        onTimeout: () => throw StateError('核心通信线程启动超时'),
      );
      _handle = handle;
    } catch (_) {
      _workerIsolate?.kill(priority: Isolate.immediate);
      _workerIsolate = null;
      await _closeMainPort();
      CoreNative.instance.destroy(handle);
      rethrow;
    } finally {
      _workerReady = null;
    }
  }

  void _onWorkerMessage(dynamic msg) {
    if (msg is! Map) {
      return;
    }
    switch (msg['type']) {
      case 'worker_port':
        final port = msg['port'] as SendPort;
        _workerPort = port;
        final ready = _workerReady;
        if (ready != null && !ready.isCompleted) {
          ready.complete(port);
        }
      case 'event':
        _eventController.add(msg['data'] as Map<String, dynamic>);
      case 'reply':
        final id = msg['id'] as int;
        final completer = _pending.remove(id);
        completer?.complete(msg['data'] as Map<String, dynamic>);
      case 'stopped':
        final stopped = _workerStopped;
        if (stopped != null && !stopped.isCompleted) {
          stopped.complete();
        }
    }
  }

  /// 执行命令；返回 {"ok": true, ...} 或 {"ok": false, "error": ...}
  Future<Map<String, dynamic>> call(
    String cmd, [
    Map<String, dynamic> args = const {},
  ]) async {
    final port = _workerPort;
    if (port == null) {
      return {'ok': false, 'error': '核心未启动'};
    }
    final id = _reqId++;
    final completer = Completer<Map<String, dynamic>>();
    _pending[id] = completer;
    port.send({'id': id, 'cmd': cmd, 'args': args});
    return completer.future.timeout(
      const Duration(seconds: 90),
      onTimeout: () {
        _pending.remove(id);
        return {'ok': false, 'error': '命令超时'};
      },
    );
  }

  Future<void> stop() async {
    final handle = _handle;
    _handle = null;
    final port = _workerPort;
    _workerPort = null;
    if (port != null) {
      _workerStopped = Completer<void>();
      port.send({'type': 'shutdown'});
      try {
        await _workerStopped!.future.timeout(const Duration(seconds: 2));
      } on TimeoutException {
        _workerIsolate?.kill(priority: Isolate.immediate);
      } finally {
        _workerStopped = null;
      }
    } else {
      _workerIsolate?.kill(priority: Isolate.immediate);
    }
    _workerIsolate = null;
    await _closeMainPort();
    if (handle != null) {
      CoreNative.instance.destroy(handle);
    }
    final pending = _pending.values.toList();
    _pending.clear();
    for (final completer in pending) {
      if (!completer.isCompleted) {
        completer.complete({'ok': false, 'error': '核心已停止'});
      }
    }
  }

  Future<void> _closeMainPort() async {
    await _mainPortSubscription?.cancel();
    _mainPortSubscription = null;
    _mainPort?.close();
    _mainPort = null;
  }

  static void _workerMain(List<Object?> args) {
    final mainPort = args[0] as SendPort;
    final handle = args[1] as int;
    final bridgeOverride = args[2] as String?;
    // 关键：Isolate.spawn 的新 isolate 是全新静态区，CoreNative.instance
    // 在这里是未加载的实例；不加载的话 call/poll 全部抛 LateInitializationError，
    // 表现为“核心日志正常但 UI 收不到任何事件/命令”。
    CoreNative.instance.load(overridePath: bridgeOverride);
    final workerPort = ReceivePort();
    mainPort.send({'type': 'worker_port', 'port': workerPort.sendPort});

    late final Timer pollTimer;
    workerPort.listen((msg) {
      if (msg is Map && msg['type'] == 'shutdown') {
        pollTimer.cancel();
        workerPort.close();
        mainPort.send({'type': 'stopped'});
        Isolate.exit();
      }
      final id = (msg as Map)['id'] as int;
      final cmd = msg['cmd'] as String;
      final args = (msg['args'] as Map).cast<String, dynamic>();
      final req = <String, dynamic>{'cmd': cmd, ...args};
      final raw = CoreNative.instance.call(handle, jsonEncode(req));
      try {
        mainPort.send({'type': 'reply', 'id': id, 'data': jsonDecode(raw)});
      } catch (_) {
        mainPort.send({
          'type': 'reply',
          'id': id,
          'data': {'ok': false, 'error': '响应解析失败: $raw'},
        });
      }
    });

    // 事件轮询（核心事件 → 主 isolate 流）
    pollTimer = Timer.periodic(const Duration(milliseconds: 150), (_) {
      final raw = CoreNative.instance.poll(handle);
      try {
        final list = jsonDecode(raw) as List<dynamic>;
        for (final e in list) {
          mainPort.send({'type': 'event', 'data': e});
        }
      } catch (_) {
        // 忽略解析失败
      }
    });
  }
}
