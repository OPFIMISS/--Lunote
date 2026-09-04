import 'dart:io';

import 'package:flutter/services.dart';

class PlatformFiles {
  PlatformFiles._();

  static const _channel = MethodChannel('com.lunote.lunote_app/platform');

  /// Android SAF 选择文件夹后复制到应用缓存，返回可供核心读取的本地路径。
  /// 这样 Download 等公共目录不会因为 DocumentsUI 的根目录限制而无法传输。
  static Future<String?> pickFolderForTransfer() async {
    if (!Platform.isAndroid) return null;
    try {
      return await _channel.invokeMethod<String>('pickFolderForTransfer');
    } catch (_) {
      return null;
    }
  }

  /// Android 原生相册选择器。返回已复制到应用缓存的本地路径。
  static Future<List<String>> pickGallery() async {
    if (!Platform.isAndroid) return const <String>[];
    try {
      final value = await _channel.invokeMethod<List<dynamic>>('pickGallery');
      return value?.whereType<String>().toList(growable: false) ??
          const <String>[];
    } catch (_) {
      return const <String>[];
    }
  }

  static Future<String?> openFile(String? path) async {
    if (path == null || path.isEmpty) return '文件路径不可用';
    if (!File(path).existsSync()) return '文件已被移动或删除';
    try {
      if (Platform.isAndroid) {
        final opened = await _channel.invokeMethod<bool>('openFile', {
          'path': path,
        });
        return opened == true ? null : '系统中没有可打开该文件的应用';
      }
      if (Platform.isWindows) {
        await Process.start(path, const [], runInShell: true);
      } else if (Platform.isMacOS) {
        await Process.start('open', [path]);
      } else if (Platform.isLinux) {
        await Process.start('xdg-open', [path]);
      }
      return null;
    } catch (e) {
      return '打开文件失败：$e';
    }
  }

  static Future<String?> openContainingFolder(
    String? path, {
    String? treeUri,
  }) async {
    if (path == null || path.isEmpty) return '文件路径不可用';
    try {
      final file = File(path);
      final dir = file.parent.path;
      if (!Directory(dir).existsSync()) return '文件所在目录已不存在';
      if (Platform.isAndroid) {
        final opened = await _channel.invokeMethod<bool>('openDirectory', {
          'path': dir,
          'treeUri': ?treeUri,
        });
        if (opened == true) return null;
        return treeUri == null
            ? 'Android 系统限制：无法直接浏览应用私有目录，文件位于 ${file.path}\n可在“设置→接收文件保存位置”选择公共目录后用系统文件管理器查看'
            : '系统中没有可用的文件管理器';
      }
      if (Platform.isWindows) {
        await Process.start('explorer.exe', ['/select,', path]);
      } else if (Platform.isMacOS) {
        await Process.start('open', ['-R', path]);
      } else if (Platform.isLinux) {
        await Process.start('xdg-open', [dir]);
      }
      return null;
    } catch (e) {
      return '打开所在文件夹失败：$e';
    }
  }
}
