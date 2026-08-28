import 'dart:convert';
import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';

import '../../core/window_ui.dart';
import '../../state/app_state.dart';
import '../lunote_theme.dart';
import '../widgets/spring_button.dart';

/// 设置页：设备名、记录导出/导入、彻底删除记录。
class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key});

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  static const _platform = MethodChannel('com.lunote.lunote_app/platform');
  late final TextEditingController _nameCtrl;
  bool _loadingDiagnostics = false;
  Map<String, dynamic>? _diagnostics;

  @override
  void initState() {
    super.initState();
    _nameCtrl = TextEditingController(text: AppState.instance.deviceName);
  }

  @override
  void dispose() {
    _nameCtrl.dispose();
    super.dispose();
  }

  Future<void> _openReceiveDirectory() async {
    final dir = await context.read<AppState>().resolvedDownloadDir();
    if (dir == null) {
      _toast('无法获取接收目录');
      return;
    }
    try {
      if (Platform.isAndroid) {
        final opened = await _platform.invokeMethod<bool>('openDirectory', {
          'path': dir,
        });
        if (opened != true) _toast('系统中没有可用的文件管理器');
      } else if (Platform.isWindows) {
        await Process.start('explorer.exe', [dir]);
      } else if (Platform.isLinux) {
        await Process.start('xdg-open', [dir]);
      } else if (Platform.isMacOS) {
        await Process.start('open', [dir]);
      }
    } catch (e) {
      _toast('打开目录失败：$e');
    }
  }

  Future<void> _showDiagnostics() async {
    setState(() => _loadingDiagnostics = true);
    try {
      final data = await context.read<AppState>().diagnostics();
      if (!mounted) return;
      setState(() => _diagnostics = data);
      await showDialog<void>(
        context: context,
        builder: (ctx) {
          final cc = LunoteColors.of(ctx);
          final d = _diagnostics ?? const <String, dynamic>{};
          return AlertDialog(
            title: const Text('设备诊断'),
            content: SizedBox(
              width: 420,
              child: SelectableText(
                '设备：${d['device_name'] ?? '-'}\n'
                '设备 ID：${d['device_id'] ?? '-'}\n'
                '监听端口：${d['tcp_port'] ?? '-'}\n'
                '在线设备：${d['peers_online'] ?? 0}/${d['peers_total'] ?? 0}\n'
                '数据目录：${d['data_dir'] ?? '-'}\n'
                '接收目录：${d['downloads_dir'] ?? '-'}\n\n'
                '发现统计：${d['discovery'] ?? '-'}',
                style: TextStyle(color: cc.moon, height: 1.55),
              ),
            ),
            actions: [TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('关闭'))],
          );
        },
      );
    } finally {
      if (mounted) setState(() => _loadingDiagnostics = false);
    }
  }

  Future<void> _export() async {
    final state = context.read<AppState>();
    final password = await _askPassword('导出记录', '设置导出密码（至少 8 位，用于加密与导入验证）');
    if (password == null || context.mounted == false) return;
    const typeGroup = XTypeGroup(
      label: 'Lunote 记录',
      extensions: ['lunote'],
      uniformTypeIdentifiers: ['public.data'],
    );
    final out = await getSaveLocation(
      suggestedName:
          'lunote-records-${DateTime.now().millisecondsSinceEpoch}.lunote',
      acceptedTypeGroups: [typeGroup],
    );
    if (out == null) return;
    final r = await state.exportRecords(password, out.path);
    if (!context.mounted) return;
    if (r['ok'] == true) {
      _toast('导出成功：${r['messages']} 条消息、${r['transfers']} 条传输记录');
    } else {
      _toast('导出失败：${r['error']}');
    }
  }

  Future<void> _import() async {
    final state = context.read<AppState>();
    final password = await _askPassword('导入记录', '输入该导出文件设置的密码');
    if (password == null || context.mounted == false) return;
    const typeGroup = XTypeGroup(
      label: 'Lunote 记录',
      extensions: ['lunote'],
      uniformTypeIdentifiers: ['public.data'],
    );
    final input = await openFile(acceptedTypeGroups: [typeGroup]);
    if (input == null) return;
    final r = await state.importRecords(password, input.path);
    if (!context.mounted) return;
    if (r['ok'] == true) {
      _toast(
        '导入成功：${r['imported_messages']} 条新消息（跳过 ${r['skipped_messages']} 条重复）',
      );
      await state.refreshConversations();
    } else {
      _toast('导入失败：${r['error']}');
    }
  }

  Future<void> _wipe() async {
    final state = context.read<AppState>();
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) {
        final cc = LunoteColors.of(ctx);
        return AlertDialog(
          title: const Text('彻底删除本地记录？'),
          content: const Text('将删除全部本地聊天与传输记录，此操作不可撤销。设备身份与信任关系不受影响。'),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: const Text('取消'),
            ),
            TextButton(
              onPressed: () => Navigator.pop(ctx, true),
              child: Text('删除', style: TextStyle(color: cc.warn)),
            ),
          ],
        );
      },
    );
    if (ok != true || !context.mounted) return;
    final r = await state.core.call('wipe_records');
    await state.refreshConversations();
    _toast(r['ok'] == true ? '已删除全部本地记录' : '删除失败：${r['error']}');
  }

  /// 一键导出诊断日志：先预览 core.log 最新内容（文件时间/大小/行数/末尾），确认后再复制
  Future<void> _exportLog() async {
    WindowUi.log('导出日志: 开始');
    final state = context.read<AppState>();
    final r = await state.core.call('data_dir');
    final dir = r['data_dir'] as String?;
    if (dir == null) {
      WindowUi.log('导出日志: data_dir 命令失败 => ${r['error'] ?? r}');
      _toast('无法获取数据目录：${r['error'] ?? '未知错误'}');
      return;
    }
    WindowUi.log('导出日志: data_dir=$dir');
    final logFile = File('$dir${Platform.pathSeparator}core.log');
    if (!logFile.existsSync()) {
      WindowUi.log('导出日志: core.log 不存在');
      _toast('暂无诊断日志（core.log 不存在）');
      return;
    }
    try {
      final stat = logFile.statSync();
      final size = stat.size;
      final mtime = stat.modified.toLocal();
      WindowUi.log('导出日志: 文件大小=$size mtime=$mtime');
      final sample = _readLogSample(logFile, size);
      if (!mounted) return;

      final doCopy = await showDialog<bool>(
        context: context,
        builder: (ctx) {
          final cc = LunoteColors.of(ctx);
          return AlertDialog(
            title: Row(
              children: [
                Icon(Icons.assignment_rounded, size: 20, color: cc.gold),
                const SizedBox(width: 8),
                const Text('诊断日志预览', style: TextStyle(fontSize: 16)),
              ],
            ),
            content: SizedBox(
              width: double.maxFinite,
              height: (MediaQuery.sizeOf(ctx).height * 0.55)
                  .clamp(240.0, 420.0)
                  .toDouble(),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    '文件时间：$mtime（本地时区）',
                    style: TextStyle(fontSize: 12, color: cc.moon),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    '大小：${_fmtSize(size)} ｜ 日志行数：${sample.lines.length}',
                    style: TextStyle(fontSize: 12, color: cc.moonDim),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    '下方是 core.log 的开头与末尾内容（可能截断），确认是最新日志后再复制',
                    style: TextStyle(fontSize: 11, color: cc.moonDim),
                  ),
                  const SizedBox(height: 8),
                  Expanded(
                    child: Container(
                      width: double.maxFinite,
                      padding: const EdgeInsets.all(10),
                      decoration: BoxDecoration(
                        color: cc.night,
                        borderRadius: BorderRadius.circular(10),
                        border: Border.all(color: cc.nightSoft),
                      ),
                      child: SingleChildScrollView(
                        child: Text(
                          sample.preview,
                          style: TextStyle(
                            fontFamily: 'monospace',
                            fontSize: 11,
                            color: cc.moon,
                            height: 1.5,
                          ),
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(ctx, false),
                child: const Text('取消'),
              ),
              SpringButton(
                weight: SpringWeight.primary,
                onTap: () => Navigator.pop(ctx, true),
                child: Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 16,
                    vertical: 8,
                  ),
                  child: Text('复制到剪贴板', style: TextStyle(color: cc.gold)),
                ),
              ),
            ],
          );
        },
      );
      if (doCopy != true || !mounted) return;
      WindowUi.log('导出日志: 用户确认复制');

      final header = [
        '月笺 Lunote 诊断日志',
        '导出时间: ${DateTime.now().toLocal()}',
        '设备ID: ${state.deviceId}',
        '数据目录: $dir',
        'core.log 文件时间: $mtime（本地时区）',
        '文件大小: ${_fmtSize(size)}',
        '===== core.log 内容（末尾，可能截断） =====',
      ].join('\n');
      await Clipboard.setData(
        ClipboardData(text: '$header\n${sample.lines.join('\n')}'),
      );
      if (!mounted) return;
      WindowUi.log('导出日志: 已复制 ${sample.lines.length} 行');
      _toast(
        '已复制 ${sample.lines.length} 行 / ${_fmtSize(size)}（文件时间 $mtime），请粘贴发给开发者',
      );
    } catch (e) {
      WindowUi.log('导出日志: 异常 => $e');
      _toast('导出日志失败：$e');
    }
  }

  /// 读取日志样本：大文件只取末尾 200KB；返回全部行 + 开头/末尾预览
  ({List<String> lines, String preview}) _readLogSample(File f, int size) {
    const maxBytes = 200 * 1024;
    String text;
    if (size > maxBytes) {
      final raf = f.openSync(mode: FileMode.read);
      raf.setPositionSync(size - maxBytes);
      final bytes = raf.readSync(maxBytes);
      raf.closeSync();
      text = utf8.decode(bytes, allowMalformed: true);
      // 截断到行首
      final nl = text.indexOf('\n');
      if (nl >= 0) {
        text = text.substring(nl + 1);
      }
    } else {
      text = f.readAsStringSync();
    }
    final lines = text.split('\n').where((l) => l.trim().isNotEmpty).toList();
    final head = lines.take(5).join('\n');
    final tail = lines.length > 15 ? lines.sublist(lines.length - 15) : lines;
    final preview =
        '── 开头 5 行 ──\n${head.isEmpty ? '(无)' : head}\n\n── 末尾 15 行 ──\n${tail.join('\n')}';
    return (lines: lines, preview: preview);
  }

  String _fmtSize(int bytes) {
    if (bytes < 1024) return '$bytes B';
    if (bytes < 1024 * 1024) return '${(bytes / 1024).toStringAsFixed(1)} KB';
    return '${(bytes / 1024 / 1024).toStringAsFixed(1)} MB';
  }

  /// 保存设备名：写核心（持久化 identity.json）后读回核对，确保下次打开不还原
  Future<void> _saveName(AppState state) async {
    final name = _nameCtrl.text.trim();
    if (name.isEmpty) {
      _toast('设备名称不能为空');
      return;
    }
    final err = await state.renameDevice(name);
    if (err != null) {
      _toast('改名失败：$err');
      return;
    }
    // 读回核对核心中的名字，确实落盘成功才提示成功
    final check = await state.core.call('identity');
    final saved = check['name'] as String? ?? '';
    if (saved == name) {
      _toast('已保存设备名称：$name');
    } else {
      _toast('警告：名称未正确保存（读回=$saved），请重试或检查数据目录');
    }
  }

  Future<String?> _askPassword(String title, String hint) {
    final ctrl = TextEditingController();
    return showDialog<String>(
      context: context,
      builder: (ctx) {
        final cc = LunoteColors.of(ctx);
        return AlertDialog(
          title: Text(title),
          content: TextField(
            controller: ctrl,
            obscureText: true,
            decoration: InputDecoration(
              hintText: hint,
              border: const OutlineInputBorder(),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(ctx),
              child: const Text('取消'),
            ),
            SpringButton(
              weight: SpringWeight.primary,
              onTap: () => Navigator.pop(ctx, ctrl.text),
              child: Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: 18,
                  vertical: 8,
                ),
                child: Text('确定', style: TextStyle(color: cc.gold)),
              ),
            ),
          ],
        );
      },
    );
  }

  void _toast(String msg) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(msg)));
  }

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    final state = context.watch<AppState>();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 20, 24, 4),
          child: Text(
            '设置',
            style: TextStyle(
              fontSize: 22,
              fontWeight: FontWeight.w700,
              color: cc.moon,
            ),
          ),
        ),
        Expanded(
          child: ListView(
            padding: const EdgeInsets.fromLTRB(18, 10, 18, 24),
            children: [
              _card(
                children: [
                  Row(
                    children: [
                      Icon(Icons.lock_rounded, size: 15, color: cc.gold),
                      const SizedBox(width: 6),
                      Text('应用锁', style: TextStyle(fontSize: 13, color: cc.moonDim)),
                    ],
                  ),
                  const SizedBox(height: 6),
                  Text('后台切换回来时要求 PIN，PIN 仅保存摘要', style: TextStyle(fontSize: 11.5, color: cc.moonDim)),
                  const SizedBox(height: 10),
                  Wrap(spacing: 10, children: [
                    SpringButton(
                      weight: SpringWeight.normal,
                      onTap: () async {
                        final state = context.read<AppState>();
                        final pin = await _askPassword('设置应用锁', '输入 4-12 位 PIN');
                        if (!mounted) return;
                        if (pin == null || pin.length < 4) { if (pin != null) _toast('PIN 至少 4 位'); return; }
                        final err = await state.setPin(pin);
                        if (!mounted) return;
                        _toast(err ?? '应用锁已开启');
                      },
                      child: _pill(Icons.lock_open_rounded, '设置 PIN'),
                    ),
                    if (context.watch<AppState>().pinEnabled)
                      SpringButton(
                        weight: SpringWeight.normal,
                        onTap: () async {
                          final err = await context.read<AppState>().setPin(null);
                          if (!mounted) return;
                          _toast(err ?? '应用锁已关闭');
                        },
                        child: _pill(Icons.lock_reset_rounded, '关闭应用锁'),
                      ),
                  ]),
                ],
              ),
              _card(
                children: [
                  Row(
                    children: [
                      Icon(Icons.network_check_rounded, size: 15, color: cc.gold),
                      const SizedBox(width: 6),
                      Text('设备诊断', style: TextStyle(fontSize: 13, color: cc.moonDim)),
                    ],
                  ),
                  const SizedBox(height: 8),
                  Text('查看监听端口、在线设备和发现统计，排查网络问题', style: TextStyle(fontSize: 11.5, color: cc.moonDim)),
                  const SizedBox(height: 10),
                  SpringButton(
                    weight: SpringWeight.normal,
                    onTap: _loadingDiagnostics ? null : _showDiagnostics,
                    child: _pill(Icons.monitor_heart_rounded, _loadingDiagnostics ? '读取中…' : '查看诊断信息'),
                  ),
                ],
              ),
              _card(
                children: [
                  Text(
                    '我的设备',
                    style: TextStyle(fontSize: 13, color: cc.moonDim),
                  ),
                  const SizedBox(height: 10),
                  Row(
                    children: [
                      Expanded(
                        child: Container(
                          padding: const EdgeInsets.symmetric(
                            horizontal: 12,
                            vertical: 2,
                          ),
                          decoration: BoxDecoration(
                            color: cc.night,
                            borderRadius: BorderRadius.circular(10),
                            border: Border.all(color: cc.nightSoft),
                          ),
                          child: TextField(
                            controller: _nameCtrl,
                            style: TextStyle(fontSize: 14, color: cc.moon),
                            decoration: const InputDecoration(
                              border: InputBorder.none,
                              hintText: '设备名称',
                              isDense: true,
                            ),
                            onSubmitted: (_) => _saveName(state),
                          ),
                        ),
                      ),
                      const SizedBox(width: 10),
                      SpringButton(
                        weight: SpringWeight.primary,
                        onTap: () => _saveName(state),
                        child: Container(
                          padding: const EdgeInsets.symmetric(
                            horizontal: 16,
                            vertical: 8,
                          ),
                          decoration: BoxDecoration(
                            color: cc.gold,
                            borderRadius: BorderRadius.circular(18),
                          ),
                          child: Text(
                            '保存',
                            style: TextStyle(
                              color: cc.night,
                              fontWeight: FontWeight.w700,
                              fontSize: 13,
                            ),
                          ),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 10),
                  Text(
                    '设备 ID：${state.deviceId}',
                    style: TextStyle(fontSize: 11.5, color: cc.moonDim),
                  ),
                ],
              ),
              _card(
                children: [
                  Text(
                    '聊天记录',
                    style: TextStyle(fontSize: 13, color: cc.moonDim),
                  ),
                  const SizedBox(height: 4),
                  Text(
                    '记录在本地加密存储（AES-256-GCM），导出使用独立密码（Argon2id）',
                    style: TextStyle(fontSize: 12, color: cc.moonDim),
                  ),
                  const SizedBox(height: 12),
                  Wrap(
                    spacing: 10,
                    runSpacing: 10,
                    children: [
                      SpringButton(
                        weight: SpringWeight.normal,
                        onTap: _export,
                        child: _pill(Icons.file_upload_rounded, '导出记录'),
                      ),
                      const SizedBox(width: 10),
                      SpringButton(
                        weight: SpringWeight.normal,
                        onTap: _import,
                        child: _pill(Icons.file_download_rounded, '导入记录'),
                      ),
                      const SizedBox(width: 20),
                      SpringButton(
                        weight: SpringWeight.normal,
                        onTap: _wipe,
                        child: _pill(
                          Icons.delete_sweep_rounded,
                          '彻底删除',
                          danger: true,
                        ),
                      ),
                    ],
                  ),
                ],
              ),
              _card(
                children: [
                  Text(
                    '接收文件保存位置',
                    style: TextStyle(fontSize: 13, color: cc.moonDim),
                  ),
                  const SizedBox(height: 4),
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Icon(Icons.folder_rounded, size: 15, color: cc.gold),
                      const SizedBox(width: 6),
                      Expanded(
                        child: Text(
                          state.defaultDownloadDir ?? '默认（数据目录\\downloads）',
                          style: TextStyle(fontSize: 12, color: cc.moonDim),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 12),
                  Row(
                    children: [
                      SpringButton(
                        weight: SpringWeight.normal,
                        onTap: () async {
                          final dir = await getDirectoryPath();
                          if (dir == null) return;
                          final err = await state.setDownloadDir(dir);
                          _toast(err == null ? '已设置接收目录，长期生效' : '设置失败：$err');
                        },
                        child: _pill(Icons.create_new_folder_rounded, '选择文件夹'),
                      ),
                      SpringButton(
                        weight: SpringWeight.normal,
                        onTap: _openReceiveDirectory,
                        child: _pill(Icons.folder_open_rounded, '打开目录'),
                      ),
                      if (Platform.isAndroid)
                        SpringButton(
                          weight: SpringWeight.normal,
                          onTap: () async {
                            final err = await state.pickReceiveFolder();
                            if (!mounted) return;
                            _toast(err ?? '已设置 Android 接收目录');
                          },
                          child: _pill(Icons.sd_storage_rounded, '选择 Android 目录'),
                        ),
                      if (state.defaultDownloadDir != null)
                        SpringButton(
                          weight: SpringWeight.normal,
                          onTap: () async {
                            final err = await state.setDownloadDir(null);
                            _toast(err == null ? '已恢复默认接收目录' : '设置失败：$err');
                          },
                          child: _pill(Icons.restore_rounded, '恢复默认'),
                        ),
                    ],
                  ),
                ],
              ),
              _card(
                children: [
                  Text(
                    '主题外观',
                    style: TextStyle(fontSize: 13, color: cc.moonDim),
                  ),
                  const SizedBox(height: 4),
                  Text(
                    '月光玻璃使用磨砂半透明表面与更柔软的动效',
                    style: TextStyle(fontSize: 11.5, color: cc.moonDim),
                  ),
                  const SizedBox(height: 10),
                  LayoutBuilder(
                    builder: (context, constraints) {
                      final narrow = constraints.maxWidth < 520;
                      return GridView.count(
                        crossAxisCount: narrow ? 2 : 4,
                        mainAxisSpacing: 8,
                        crossAxisSpacing: 8,
                        childAspectRatio: narrow ? 2.8 : 2.45,
                        shrinkWrap: true,
                        physics: const NeverScrollableScrollPhysics(),
                        children: [
                          _themeOption(
                            state,
                            'dark',
                            Icons.nightlight_round,
                            '深色',
                          ),
                          _themeOption(
                            state,
                            'light',
                            Icons.wb_sunny_rounded,
                            '浅色',
                          ),
                          _themeOption(
                            state,
                            'system',
                            Icons.settings_suggest_rounded,
                            '跟随系统',
                          ),
                          _themeOption(
                            state,
                            'glass',
                            Icons.blur_on_rounded,
                            '月光玻璃',
                          ),
                        ],
                      );
                    },
                  ),
                ],
              ),
              _card(
                children: [
                  Row(
                    children: [
                      Icon(Icons.rule_rounded, size: 15, color: cc.gold),
                      const SizedBox(width: 6),
                      Text('文件冲突处理', style: TextStyle(fontSize: 13, color: cc.moonDim)),
                    ],
                  ),
                  const SizedBox(height: 8),
                  Text('接收同名文件时的默认行为，双端同步保存', style: TextStyle(fontSize: 11.5, color: cc.moonDim)),
                  const SizedBox(height: 8),
                  DropdownButtonFormField<String>(
                    initialValue: state.conflictPolicy,
                    items: const [
                      DropdownMenuItem(value: 'rename', child: Text('自动重命名（推荐）')),
                      DropdownMenuItem(value: 'overwrite', child: Text('覆盖已有文件')),
                      DropdownMenuItem(value: 'skip', child: Text('跳过已有文件')),
                    ],
                    onChanged: (v) async {
                      if (v == null) return;
                      final error = await state.setConflictPolicy(v);
                      if (error != null) _toast('保存失败：$error');
                    },
                  ),
                ],
              ),
              _card(
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              '自动信任',
                              style: TextStyle(fontSize: 13, color: cc.moonDim),
                            ),
                            SizedBox(height: 4),
                            Text(
                              '新设备与已信任设备“同名且同 IP”时自动信任，无需重复确认。\n仅建议在家庭/可信局域网开启；公共网络建议关闭',
                              style: TextStyle(
                                fontSize: 11.5,
                                height: 1.5,
                                color: cc.moonDim,
                              ),
                            ),
                          ],
                        ),
                      ),
                      Switch(
                        value: state.autoTrust,
                        onChanged: (v) async {
                          final error = await state.setAutoTrust(v);
                          if (error != null) _toast('自动信任保存失败：$error');
                        },
                        activeTrackColor: cc.goldDeep,
                        activeThumbColor: cc.night,
                      ),
                    ],
                  ),
                ],
              ),
              _card(
                children: [
                  Row(
                    children: [
                      Icon(Icons.bug_report_rounded, size: 15, color: cc.gold),
                      SizedBox(width: 6),
                      Text(
                        '调试模式',
                        style: TextStyle(fontSize: 13, color: cc.moonDim),
                      ),
                    ],
                  ),
                  const SizedBox(height: 4),
                  Text(
                    '一键复制发现/会话/传输运行日志到剪贴板，粘贴发给开发者即可定位问题',
                    style: TextStyle(fontSize: 12, color: cc.moonDim),
                  ),
                  const SizedBox(height: 12),
                  SpringButton(
                    weight: SpringWeight.normal,
                    onTap: _exportLog,
                    child: _pill(Icons.copy_rounded, '一键导出诊断日志'),
                  ),
                ],
              ),
              _card(
                children: [
                  Text('关于', style: TextStyle(fontSize: 13, color: cc.moonDim)),
                  const SizedBox(height: 8),
                  Text(
                    '月笺 Lunote v1.1.0\n无云端 · 无账号 · 无互联网依赖\n数据只在你和设备之间直接传输',
                    style: TextStyle(
                      fontSize: 12.5,
                      height: 1.6,
                      color: cc.moon,
                    ),
                  ),
                ],
              ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _card({required List<Widget> children}) {
    final cc = LunoteColors.of(context);
    return Container(
      margin: const EdgeInsets.symmetric(vertical: 6),
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: cc.nightRaised,
        borderRadius: BorderRadius.circular(16),
        border: cc.isGlass ? Border.all(color: const Color(0xAFFFFFFF)) : null,
        boxShadow: cc.isGlass
            ? const [
                BoxShadow(
                  color: Color(0x1F38536A),
                  blurRadius: 18,
                  offset: Offset(0, 5),
                ),
              ]
            : null,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: children,
      ),
    );
  }

  Widget _themeOption(
    AppState state,
    String value,
    IconData icon,
    String label,
  ) {
    final cc = LunoteColors.of(context);
    final selected = state.themeMode == value;
    return SpringButton(
      weight: SpringWeight.normal,
      onTap: () async {
        final error = await state.setTheme(value);
        if (error != null) _toast('主题保存失败：$error');
      },
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 180),
        height: 44,
        padding: const EdgeInsets.symmetric(horizontal: 8),
        decoration: BoxDecoration(
          color: selected ? cc.goldDeep : cc.night,
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: selected ? cc.gold : cc.nightSoft),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              selected ? Icons.check_rounded : icon,
              size: 18,
              color: selected ? cc.night : cc.moon,
            ),
            const SizedBox(width: 6),
            Flexible(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 12.5,
                  fontWeight: FontWeight.w600,
                  color: selected ? cc.night : cc.moon,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _pill(IconData icon, String label, {bool danger = false}) {
    final cc = LunoteColors.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
      decoration: BoxDecoration(
        color: danger ? const Color(0x22E58A7A) : cc.nightSoft,
        borderRadius: BorderRadius.circular(18),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 15, color: danger ? cc.warn : cc.gold),
          const SizedBox(width: 6),
          Text(
            label,
            style: TextStyle(fontSize: 12.5, color: danger ? cc.warn : cc.moon),
          ),
        ],
      ),
    );
  }
}
