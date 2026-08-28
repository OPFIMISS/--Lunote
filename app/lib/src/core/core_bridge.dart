import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

/// lunote_bridge 原生库的 FFI 绑定（Windows dll / Android so / Linux so）。
class CoreNative {
  CoreNative._();

  static final CoreNative instance = CoreNative._();

  /// 可空：worker isolate 中的实例是全新静态区，需先 load 才能使用。
  DynamicLibrary? _lib;
  late final _CreateDart _createFn;
  late final _CallDart _callFn;
  late final _PollDart _pollFn;
  late final _DestroyDart _destroyFn;
  late final _FreeDart _freeFn;

  /// 加载原生库。overridePath 用于开发期指定 dll 位置。
  /// 幂等：已加载则直接返回（主 isolate 与 worker isolate 各自持有实例）。
  void load({String? overridePath}) {
    if (_lib != null) return;
    final candidates = <String>[?overridePath, ...defaultCandidates()];
    DynamicLibrary? lib;
    String? lastError;
    for (final c in candidates) {
      try {
        lib = DynamicLibrary.open(c);
        break;
      } catch (e) {
        lastError = e.toString();
      }
    }
    if (lib == null) {
      throw StateError('无法加载 lunote_bridge：$lastError\n尝试路径：$candidates');
    }
    _lib = lib;
    _createFn = _lib!
        .lookup<NativeFunction<_CreateNative>>('lunote_create')
        .asFunction<_CreateDart>();
    _callFn = _lib!
        .lookup<NativeFunction<_CallNative>>('lunote_call')
        .asFunction<_CallDart>();
    _pollFn = _lib!
        .lookup<NativeFunction<_PollNative>>('lunote_poll')
        .asFunction<_PollDart>();
    _destroyFn = _lib!
        .lookup<NativeFunction<_DestroyNative>>('lunote_destroy')
        .asFunction<_DestroyDart>();
    _freeFn = _lib!
        .lookup<NativeFunction<_FreeNative>>('lunote_free_string')
        .asFunction<_FreeDart>();
  }

  static List<String> defaultCandidates() {
    if (Platform.isWindows) {
      return [
        'lunote_bridge.dll',
        ?Platform.environment['LUNOTE_BRIDGE_PATH'],
        // 构建输出（Release 与 Debug）
        'build/windows/x64/runner/Release/lunote_bridge.dll',
        'build/windows/x64/runner/Debug/lunote_bridge.dll',
      ];
    }
    if (Platform.isAndroid) {
      return ['liblunote_bridge.so'];
    }
    return ['liblunote_bridge.so'];
  }

  /// 创建核心实例，返回句柄（<=0 失败）
  int create(String configJson) {
    final c = configJson.toNativeUtf8();
    final handle = _createFn(c);
    malloc.free(c);
    return handle;
  }

  /// 同步执行命令，返回响应 JSON（命令语言与 CLI 控制文件一致）
  String call(int handle, String cmdJson) {
    final c = cmdJson.toNativeUtf8();
    final ptr = _callFn(handle, c);
    malloc.free(c);
    if (ptr == nullptr) {
      return '{"ok":false,"error":"原生层返回空"}';
    }
    final s = ptr.toDartString();
    _freeFn(ptr);
    return s;
  }

  /// 拉取待处理事件（JSON 数组）
  String poll(int handle) {
    final ptr = _pollFn(handle);
    if (ptr == nullptr) {
      return '[]';
    }
    final s = ptr.toDartString();
    _freeFn(ptr);
    return s;
  }

  void destroy(int handle) {
    _destroyFn(handle);
  }
}

// 原生签名（dart:ffi 类型）
typedef _CreateNative = Int64 Function(Pointer<Utf8>);
typedef _CallNative = Pointer<Utf8> Function(Int64, Pointer<Utf8>);
typedef _PollNative = Pointer<Utf8> Function(Int64);
typedef _DestroyNative = Void Function(Int64);
typedef _FreeNative = Void Function(Pointer<Utf8>);

// Dart 侧签名
typedef _CreateDart = int Function(Pointer<Utf8>);
typedef _CallDart = Pointer<Utf8> Function(int, Pointer<Utf8>);
typedef _PollDart = Pointer<Utf8> Function(int);
typedef _DestroyDart = void Function(int);
typedef _FreeDart = void Function(Pointer<Utf8>);
