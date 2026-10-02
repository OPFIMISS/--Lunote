import 'dart:io';
import 'dart:convert';
import 'dart:ui' as ui;

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';

import '../../core/models.dart';
import '../../state/app_state.dart';
import '../lunote_theme.dart';

class NotesPage extends StatefulWidget {
  const NotesPage({super.key});
  @override
  State<NotesPage> createState() => _NotesPageState();
}

class _NotesPageState extends State<NotesPage> with WidgetsBindingObserver {
  final Set<String> _revealed = {};
  final Set<String> _folded = {};
  bool _compact = false;
  String _group = '';
  BuildContext? _editorContext;
  TextEditingController? _editorBody;
  BuildContext? _passwordContext;
  TextEditingController? _passwordController;
  bool _foreground = true;
  int _privacyEpoch = 0;
  final _channel = const MethodChannel('com.lunote.lunote_app/platform');

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    _foreground = state == AppLifecycleState.resumed;
    if (state != AppLifecycleState.resumed) {
      _privacyEpoch++;
      _editorBody?.clear();
      _passwordController?.clear();
      final passwordContext = _passwordContext;
      if (passwordContext != null && passwordContext.mounted) {
        Navigator.of(passwordContext).pop();
      }
      final editor = _editorContext;
      if (editor != null && editor.mounted) Navigator.of(editor).pop();
      if (mounted) setState(_revealed.clear);
    }
  }

  void _toast(String text) {
    if (mounted) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
    }
  }

  Future<String?> _password(String title) async {
    final controller = TextEditingController();
    _passwordController = controller;
    final result = await showDialog<String>(
      context: context,
      builder: (context) {
        _passwordContext = context;
        return AlertDialog(
          title: Text(title),
          content: TextField(
            controller: controller,
            obscureText: true,
            autofocus: true,
            enableSuggestions: false,
            autocorrect: false,
            decoration: const InputDecoration(labelText: '密码'),
            onSubmitted: (_) => Navigator.pop(context, controller.text),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('取消'),
            ),
            TextButton(
              onPressed: () => Navigator.pop(context, controller.text),
              child: const Text('确认'),
            ),
          ],
        );
      },
    );
    _passwordContext = null;
    _passwordController = null;
    controller.clear();
    controller.dispose();
    return result;
  }

  String _credentialTag(NoteItem note) {
    final locked = (note.data['locked'] as Map).cast<String, dynamic>();
    return jsonEncode({'context': locked['context'], 'salt': locked['salt']});
  }

  Future<String?> _notePassword(NoteItem note) async {
    if (Platform.isAndroid) {
      try {
        final available = await _channel.invokeMethod<bool>(
          'noteCredentialAvailable',
          {'noteId': note.id, 'tag': _credentialTag(note)},
        );
        if (!mounted) return null;
        if (available == true) {
          final choice = await showDialog<String>(
            context: context,
            builder: (context) => AlertDialog(
              title: const Text('解锁笔记'),
              actions: [
                TextButton(
                  onPressed: () => Navigator.pop(context),
                  child: const Text('取消'),
                ),
                TextButton(
                  onPressed: () => Navigator.pop(context, 'password'),
                  child: const Text('笔记密码'),
                ),
                FilledButton.icon(
                  onPressed: () => Navigator.pop(context, 'system'),
                  icon: const Icon(Icons.fingerprint_rounded),
                  label: const Text('系统验证'),
                ),
              ],
            ),
          );
          if (choice == null || !mounted) return null;
          if (choice == 'system') {
            return await _channel.invokeMethod<String>('unlockNoteCredential', {
              'noteId': note.id,
              'tag': _credentialTag(note),
            });
          }
        }
      } catch (_) {
        _toast('系统验证暂不可用，请输入笔记密码');
      }
    }
    if (!mounted) return null;
    return _password('解锁笔记');
  }

  Future<bool> _save(
    Map<String, dynamic> note, {
    String? password,
    String? newPassword,
  }) async {
    final state = context.read<AppState>();
    final result = await state.core.call('save_note', {
      'note': note,
      'password': ?password,
      'new_password': ?newPassword,
    });
    if (result['ok'] != true) {
      _toast(result['error'] as String? ?? '保存失败');
      return false;
    }
    await state.refreshNotes();
    return true;
  }

  Future<void> _edit([NoteItem? note]) async {
    final state = context.read<AppState>();
    String? password;
    var body = note?.body ?? '';
    if (note?.locked == true) {
      password = await _notePassword(note!);
      if (password == null || !mounted || !_foreground) return;
      final epoch = _privacyEpoch;
      final result = await state.core.call('unlock_note', {
        'note_id': note.id,
        'password': password,
      });
      if (!mounted || !_foreground || epoch != _privacyEpoch) return;
      if (result['ok'] != true) {
        _toast(result['error'] as String? ?? '解锁失败');
        return;
      }
      body = result['body'] as String;
    }
    if (!mounted || !_foreground) return;
    final titleController = TextEditingController(text: note?.title ?? '');
    final bodyController = TextEditingController(text: body);
    final groupController = TextEditingController(
      text: note?.group ?? (_group == '' ? '' : _group),
    );
    final lockController = TextEditingController();
    var pinned = note?.pinned ?? false;
    var shielded = note?.shielded ?? false;
    var locked = note?.locked ?? false;
    var busy = false;
    _editorBody = bodyController;
    await showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (dialogContext) {
        _editorContext = dialogContext;
        return StatefulBuilder(
          builder: (context, setDialogState) => AlertDialog(
            title: Row(
              children: [
                Expanded(child: Text(note == null ? '新建笔记' : '编辑笔记')),
                IconButton(
                  onPressed: busy ? null : () => Navigator.pop(context),
                  tooltip: '关闭',
                  icon: const Icon(Icons.close_rounded),
                ),
              ],
            ),
            content: SizedBox(
              width: 560,
              child: SingleChildScrollView(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    TextField(
                      controller: titleController,
                      maxLength: 64,
                      decoration: const InputDecoration(labelText: '标题'),
                    ),
                    TextField(
                      controller: bodyController,
                      minLines: 5,
                      maxLines: 12,
                      maxLength: 16000,
                      enableSuggestions: !locked,
                      autocorrect: !locked,
                      decoration: const InputDecoration(labelText: '内容'),
                    ),
                    TextField(
                      controller: groupController,
                      maxLength: 32,
                      decoration: const InputDecoration(labelText: '分组'),
                    ),
                    SwitchListTile(
                      contentPadding: EdgeInsets.zero,
                      title: const Text('置顶'),
                      value: pinned,
                      onChanged: busy
                          ? null
                          : (value) => setDialogState(() => pinned = value),
                    ),
                    SwitchListTile(
                      contentPadding: EdgeInsets.zero,
                      title: const Text('防窥模糊'),
                      value: shielded,
                      onChanged: busy
                          ? null
                          : (value) => setDialogState(() => shielded = value),
                    ),
                    SwitchListTile(
                      contentPadding: EdgeInsets.zero,
                      title: const Text('密码加密'),
                      value: locked,
                      onChanged: busy
                          ? null
                          : (value) => setDialogState(() => locked = value),
                    ),
                    if (locked)
                      TextField(
                        controller: lockController,
                        obscureText: true,
                        enableSuggestions: false,
                        autocorrect: false,
                        decoration: InputDecoration(
                          labelText: note?.locked == true
                              ? '新密码（留空保持不变）'
                              : '设置密码（至少8位）',
                        ),
                      ),
                  ],
                ),
              ),
            ),
            actions: [
              FilledButton.icon(
                icon: busy
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.save_outlined),
                label: const Text('保存'),
                onPressed: busy
                    ? null
                    : () async {
                        final newPassword = locked
                            ? (lockController.text.isEmpty &&
                                      note?.locked == true
                                  ? null
                                  : lockController.text)
                            : (note?.locked == true ? '' : null);
                        if (locked &&
                            note?.locked != true &&
                            (newPassword?.length ?? 0) < 8) {
                          _toast('密码至少8位');
                          return;
                        }
                        setDialogState(() => busy = true);
                        final data = <String, dynamic>{
                          if (note != null) ...note.data,
                          'id': note?.id ?? '',
                          'title': titleController.text.trim(),
                          'body': bodyController.text,
                          'clock': note?.data['clock'] ?? <String, dynamic>{},
                          'locked': note?.data['locked'],
                          'deleted': false,
                          'pinned': pinned,
                          'group': groupController.text.trim(),
                          'order': note?.order ?? state.notes.length,
                          'shielded': shielded,
                        };
                        final success = await _save(
                          data,
                          password: password,
                          newPassword: newPassword,
                        );
                        if (!context.mounted) return;
                        if (success) {
                          Navigator.pop(context);
                        } else {
                          setDialogState(() => busy = false);
                        }
                      },
              ),
            ],
          ),
        );
      },
    );
    _editorContext = null;
    _editorBody = null;
    bodyController.clear();
    lockController.clear();
    titleController.dispose();
    bodyController.dispose();
    groupController.dispose();
    lockController.dispose();
  }

  Future<void> _selectPeers() async {
    final state = context.read<AppState>();
    final selected = Set<String>.from(state.noteSyncPeers);
    await showDialog<void>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, setDialogState) => AlertDialog(
          title: const Text('笔记同步设备'),
          content: SizedBox(
            width: 400,
            child: SingleChildScrollView(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  for (final peer in state.trusted.values.where(
                    (peer) => peer.trusted,
                  ))
                    CheckboxListTile(
                      value: selected.contains(peer.deviceId),
                      title: Text(state.peerName(peer.deviceId)),
                      subtitle: Text(
                        state.peer(peer.deviceId)?.online == true ? '在线' : '离线',
                      ),
                      onChanged: (value) => setDialogState(() {
                        if (value == true) {
                          selected.add(peer.deviceId);
                        } else {
                          selected.remove(peer.deviceId);
                        }
                      }),
                    ),
                  if (!state.trusted.values.any((peer) => peer.trusted))
                    const Text('暂无可信设备'),
                ],
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () async {
                final result = await state.core.call('set_note_peers', {
                  'device_ids': selected.toList(),
                });
                if (result['ok'] != true) {
                  _toast(result['error'] as String? ?? '设置失败');
                  return;
                }
                await state.refreshNotes();
                if (context.mounted) Navigator.pop(context);
              },
              child: const Text('保存'),
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _backup(bool importing) async {
    try {
      final state = context.read<AppState>();
      final password = await _password(importing ? '解密笔记备份' : '设置备份密码');
      if (password == null || !mounted) return;
      String? path;
      String? tree;
      const types = [
        XTypeGroup(label: '月笺笔记', extensions: ['lunotes']),
      ];
      if (importing) {
        path = (await openFile(acceptedTypeGroups: types))?.path;
      } else if (Platform.isAndroid) {
        tree = await _channel.invokeMethod<String>('pickReceiveFolder');
        if (tree == null) return;
        final dir = await state.core.call('data_dir');
        path =
            '${dir['data_dir']}/notes-${DateTime.now().millisecondsSinceEpoch}.lunotes';
      } else {
        path = (await getSaveLocation(
          suggestedName: 'notes.lunotes',
          acceptedTypeGroups: types,
        ))?.path;
      }
      if (path == null) return;
      final result = await state.core.call(
        importing ? 'import_notes' : 'export_notes',
        {'password': password, 'path': path},
      );
      if (result['ok'] != true) {
        _toast(result['error'] as String? ?? '备份操作失败');
        return;
      }
      if (tree != null) {
        try {
          final exported = await _channel.invokeMethod<String>('exportToTree', {
            'path': path,
            'treeUri': tree,
          });
          if (exported == null) {
            _toast('备份写入公共目录失败');
            return;
          }
        } finally {
          await File(path).delete();
        }
      }
      await state.refreshNotes();
      _toast(importing ? '已导入加密备份' : '已保存加密备份');
    } catch (error) {
      _toast('备份操作失败：$error');
    }
  }

  Future<void> _action(String action, NoteItem note) async {
    switch (action) {
      case 'edit':
        await _edit(note);
      case 'pin':
        await _save({...note.data, 'pinned': !note.pinned});
      case 'fold':
        setState(() {
          if (_compact) {
            _compact = false;
            _folded
              ..clear()
              ..addAll(context.read<AppState>().notes.map((item) => item.id))
              ..remove(note.id);
          } else if (!_folded.add(note.id)) {
            _folded.remove(note.id);
          }
        });
      case 'copy':
        final copyEpoch = _privacyEpoch;
        var body = note.body;
        if (note.locked) {
          final password = await _notePassword(note);
          if (password == null || !mounted) return;
          final result = await context.read<AppState>().core.call(
            'unlock_note',
            {'note_id': note.id, 'password': password},
          );
          if (!mounted || !_foreground || copyEpoch != _privacyEpoch) return;
          if (result['ok'] != true) {
            _toast(result['error'] as String? ?? '解锁失败');
            return;
          }
          body = result['body'] as String;
        }
        if (!mounted || !_foreground || copyEpoch != _privacyEpoch) return;
        await Clipboard.setData(ClipboardData(text: body));
        _toast('已复制笔记');
      case 'system':
        final password = await _password('验证笔记密码');
        if (password == null || !mounted) return;
        final result = await context.read<AppState>().core.call('unlock_note', {
          'note_id': note.id,
          'password': password,
        });
        if (result['ok'] != true) {
          _toast(result['error'] as String? ?? '密码错误');
          return;
        }
        try {
          final remembered = await _channel.invokeMethod<String>(
            'rememberNoteCredential',
            {
              'noteId': note.id,
              'tag': _credentialTag(note),
              'password': password,
            },
          );
          if (remembered != null) _toast('本设备已启用系统解锁');
        } catch (error) {
          _toast('启用系统解锁失败：$error');
        }
      case 'forget':
        await _channel.invokeMethod<bool>('forgetNoteCredential', {
          'noteId': note.id,
          'tag': _credentialTag(note),
        });
        _toast('已停用本设备的系统解锁');
      case 'delete':
        final confirm = await showDialog<bool>(
          context: context,
          builder: (context) => AlertDialog(
            title: const Text('删除笔记？'),
            content: Text('「${note.title}」的删除会同步到所选设备，无法撤销。'),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(context, false),
                child: const Text('取消'),
              ),
              TextButton(
                onPressed: () => Navigator.pop(context, true),
                child: const Text('删除'),
              ),
            ],
          ),
        );
        if (confirm == true && mounted) {
          await _save({
            ...note.data,
            'deleted': true,
            if (note.locked) 'body': null,
          });
        }
    }
  }

  Future<void> _reorder(String source, String target) async {
    final state = context.read<AppState>();
    final ordered = state.notes.where((note) => !note.deleted).toList()
      ..sort(_compare);
    final from = ordered.indexWhere((note) => note.id == source);
    final to = ordered.indexWhere((note) => note.id == target);
    if (from < 0 ||
        to < 0 ||
        from == to ||
        ordered[from].pinned != ordered[to].pinned) {
      return;
    }
    ordered.insert(to, ordered.removeAt(from));
    final result = await state.core.call('reorder_notes', {
      'versions': ordered
          .map((note) => {'id': note.id, 'clock': note.data['clock']})
          .toList(),
    });
    if (result['ok'] != true) _toast(result['error'] as String? ?? '排序失败');
    await state.refreshNotes();
  }

  int _compare(NoteItem a, NoteItem b) {
    if (a.pinned != b.pinned) return a.pinned ? -1 : 1;
    final order = a.order.compareTo(b.order);
    return order == 0 ? a.id.compareTo(b.id) : order;
  }

  Widget _card(NoteItem note, double width) {
    final cc = LunoteColors.of(context);
    final folded = _compact || _folded.contains(note.id);
    final shielded = note.shielded && !_revealed.contains(note.id);
    final content = Container(
      width: width,
      height: folded ? 80 : width,
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: cc.nightRaised,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: cc.nightSoft),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              if (note.pinned)
                Padding(
                  padding: const EdgeInsets.only(right: 5),
                  child: Icon(Icons.push_pin_rounded, size: 16, color: cc.gold),
                ),
              Expanded(
                child: Text(
                  note.title.isEmpty ? '无标题' : note.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontWeight: FontWeight.w600, color: cc.moon),
                ),
              ),
              if (folded && note.locked)
                Icon(Icons.lock_outline_rounded, size: 18, color: cc.moonDim),
              if (folded)
                IconButton(
                  tooltip: '展开',
                  onPressed: () => _action('fold', note),
                  icon: const Icon(Icons.expand_more_rounded),
                ),
              PopupMenuButton<String>(
                tooltip: '笔记操作',
                padding: EdgeInsets.zero,
                onSelected: (action) => _action(action, note),
                itemBuilder: (_) => [
                  const PopupMenuItem(
                    value: 'edit',
                    child: Text('编辑 / 移动 / 加密'),
                  ),
                  PopupMenuItem(
                    value: 'pin',
                    child: Text(note.pinned ? '取消置顶' : '置顶'),
                  ),
                  PopupMenuItem(
                    value: 'fold',
                    child: Text(folded ? '展开' : '收起'),
                  ),
                  const PopupMenuItem(value: 'copy', child: Text('复制内容')),
                  if (Platform.isAndroid && note.locked)
                    const PopupMenuItem(
                      value: 'system',
                      child: Text('启用本机指纹 / 锁屏解锁'),
                    ),
                  if (Platform.isAndroid && note.locked)
                    const PopupMenuItem(
                      value: 'forget',
                      child: Text('停用本机系统解锁'),
                    ),
                  const PopupMenuItem(value: 'delete', child: Text('删除')),
                ],
              ),
            ],
          ),
          if (!folded) ...[
            Expanded(
              child: note.locked
                  ? Center(
                      child: Icon(
                        Icons.lock_outline_rounded,
                        size: 42,
                        color: cc.moonDim,
                      ),
                    )
                  : ClipRect(
                      child: ExcludeSemantics(
                        excluding: shielded,
                        child: ImageFiltered(
                          imageFilter: ui.ImageFilter.blur(
                            sigmaX: shielded ? 7 : 0,
                            sigmaY: shielded ? 7 : 0,
                          ),
                          child: Align(
                            alignment: Alignment.topLeft,
                            child: Text(
                              note.body,
                              maxLines: 8,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(color: cc.moon, height: 1.5),
                            ),
                          ),
                        ),
                      ),
                    ),
            ),
            const SizedBox(height: 6),
            Row(
              children: [
                Expanded(
                  child: Text(
                    note.group,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(color: cc.moonDim, fontSize: 12),
                  ),
                ),
                if (note.shielded)
                  Icon(
                    Icons.visibility_off_outlined,
                    size: 16,
                    color: cc.moonDim,
                  ),
                IconButton(
                  tooltip: '收起',
                  visualDensity: VisualDensity.compact,
                  onPressed: () => _action('fold', note),
                  icon: const Icon(Icons.expand_less_rounded),
                ),
              ],
            ),
          ],
        ],
      ),
    );
    return DragTarget<String>(
      onAcceptWithDetails: (details) => _reorder(details.data, note.id),
      builder: (context, candidates, rejected) => LongPressDraggable<String>(
        data: note.id,
        feedback: Material(
          color: Colors.transparent,
          child: Opacity(opacity: 0.85, child: content),
        ),
        childWhenDragging: Opacity(opacity: 0.3, child: content),
        child: AnimatedScale(
          scale: candidates.isEmpty ? 1 : 1.025,
          duration: const Duration(milliseconds: 180),
          child: GestureDetector(
            onDoubleTap: note.shielded
                ? () => setState(() => _revealed.add(note.id))
                : null,
            onTap: shielded && !note.locked ? null : () => _edit(note),
            child: AnimatedSize(
              duration: MediaQuery.disableAnimationsOf(context)
                  ? Duration.zero : const Duration(milliseconds:220),
              curve: Curves.easeOutCubic,
              alignment: Alignment.topCenter,
              child: content,
            ),
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final cc = LunoteColors.of(context);
    final groups =
        state.notes
            .where((note) => !note.deleted && note.group.isNotEmpty)
            .map((note) => note.group)
            .toSet()
            .toList()
          ..sort();
    final group = groups.contains(_group) ? _group : '';
    final notes =
        state.notes
            .where(
              (note) => !note.deleted && (group.isEmpty || note.group == group),
            )
            .toList()
          ..sort(_compare);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 12, 8, 8),
          child: Wrap(
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: 6,
            children: [
              Text(
                '笔记',
                style: TextStyle(
                  fontSize: 22,
                  fontWeight: FontWeight.w700,
                  color: cc.moon,
                ),
              ),
              SizedBox(
                width: 140,
                child: DropdownButton<String>(
                  value: group,
                  isExpanded: true,
                  items: [
                    const DropdownMenuItem(value: '', child: Text('全部分组')),
                    for (final item in groups)
                      DropdownMenuItem(
                        value: item,
                        child: Text(item, overflow: TextOverflow.ellipsis),
                      ),
                  ],
                  onChanged: (value) => setState(() => _group = value ?? ''),
                ),
              ),
              IconButton(
                tooltip: '新建笔记',
                onPressed: () => _edit(),
                icon: const Icon(Icons.add_rounded),
              ),
              IconButton(
                tooltip: '同步设备',
                onPressed: _selectPeers,
                icon: const Icon(Icons.sync_rounded),
              ),
              IconButton(
                tooltip: _compact ? '方形卡片' : '仅显示标题',
                onPressed: () => setState(() => _compact = !_compact),
                icon: Icon(
                  _compact ? Icons.grid_view_rounded : Icons.view_list_rounded,
                ),
              ),
              PopupMenuButton<String>(
                tooltip: '备份',
                onSelected: (action) => _backup(action == 'import'),
                itemBuilder: (_) => [
                  const PopupMenuItem(value: 'export', child: Text('导出加密备份')),
                  const PopupMenuItem(value: 'import', child: Text('导入加密备份')),
                ],
              ),
            ],
          ),
        ),
        Expanded(
          child: notes.isEmpty
              ? Center(
                  child: Text('暂无笔记', style: TextStyle(color: cc.moonDim)),
                )
              : LayoutBuilder(
                  builder: (context, constraints) {
                    final columns = _compact
                        ? 1
                        : (constraints.maxWidth / 240).floor().clamp(1, 5);
                    final width =
                        (constraints.maxWidth - 32 - (columns - 1) * 12) /
                        columns;
                    return SingleChildScrollView(
                      padding: const EdgeInsets.fromLTRB(16, 8, 16, 24),
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          for (var column = 0; column < columns; column++) ...[
                            if (column > 0) const SizedBox(width: 12),
                            SizedBox(
                              width: width,
                              child: Column(
                                children: [
                                  for (
                                    var index = column;
                                    index < notes.length;
                                    index += columns
                                  )
                                    Padding(
                                      key: ValueKey(notes[index].id),
                                      padding: const EdgeInsets.only(
                                        bottom: 12,
                                      ),
                                      child: _card(notes[index], width),
                                    ),
                                ],
                              ),
                            ),
                          ],
                        ],
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }
}
