import 'dart:io';

import 'package:desktop_drop/desktop_drop.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';

import '../../core/models.dart';
import '../../core/platform_files.dart';
import '../../state/app_state.dart';
import '../formatters.dart';
import '../file_types.dart';
import '../lunote_theme.dart';
import '../widgets/message_bubble.dart';
import '../widgets/spring_button.dart';
import '../widgets/timeline_entrance.dart';
import '../widgets/transfer_tile.dart';
import 'media_preview_page.dart';

/// 对话页：双向文字/链接/文件/文件夹；拖放发送；传输状态来自核心事件。
class ChatPage extends StatefulWidget {
  const ChatPage({super.key, required this.deviceId, this.onBack});

  final String deviceId;

  /// 宽屏内嵌模式（无 Navigator 可 pop）时的返回回调；为 null 且不可 pop 时不显示返回按钮
  final VoidCallback? onBack;

  @override
  State<ChatPage> createState() => _ChatPageState();
}

class _ChatPageState extends State<ChatPage> {
  final TextEditingController _input = TextEditingController();
  final FocusNode _inputFocus = FocusNode();
  final ScrollController _scroll = ScrollController();
  bool _dragging = false;
  int _lastTimelineLength = -1;

  @override
  void dispose() {
    _input.dispose();
    _inputFocus.dispose();
    _scroll.dispose();
    super.dispose();
  }

  void _scrollToBottom() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_scroll.hasClients) {
        _scroll.jumpTo(_scroll.position.maxScrollExtent);
      }
    });
  }

  Future<void> _sendText() async {
    final text = _input.text.trim();
    if (text.isEmpty) return;
    _input.clear();
    final isLink = text.startsWith('http://') || text.startsWith('https://');
    final state = context.read<AppState>();
    final err = isLink
        ? await state.sendLink(widget.deviceId, text)
        : await state.sendText(widget.deviceId, text);
    if (err != null && mounted) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(err)));
    }
    _scrollToBottom();
  }

  Future<void> _pickFiles() async {
    const typeGroup = XTypeGroup(label: '全部文件', extensions: []);
    final result = await openFiles(acceptedTypeGroups: [typeGroup]);
    for (final f in result) {
      await _sendPath(f.path);
    }
  }

  Future<void> _pickFolder() async {
    final dir = Platform.isAndroid
        ? await PlatformFiles.pickFolderForTransfer()
        : await getDirectoryPath();
    if (dir != null) {
      await _sendPath(dir);
    }
  }

  Future<void> _sendPath(String path) async {
    final state = context.read<AppState>();
    final err = await state.sendFile(widget.deviceId, path);
    if (err != null && mounted) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(err)));
    }
  }

  void _insertDroppedText(String text) {
    final value = text.trim();
    if (value.isEmpty) return;
    _input.value = TextEditingValue(
      text: value,
      selection: TextSelection.collapsed(offset: value.length),
    );
    _inputFocus.requestFocus();
    ScaffoldMessenger.of(context)
        .showSnackBar(const SnackBar(content: Text('已将拖入的文字填入输入框')));
  }

  Future<void> _showClipboard() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    if (!mounted) return;
    final text = data?.text?.trim() ?? '';
    if (text.isEmpty) {
      ScaffoldMessenger.of(context)
          .showSnackBar(const SnackBar(content: Text('系统剪贴板中没有可用文字')));
      return;
    }
    await showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (sheetContext) {
        final cc = LunoteColors.of(sheetContext);
        return SafeArea(
          top: false,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(20, 0, 20, 18),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  '系统剪贴板',
                  style: TextStyle(
                    fontSize: 17,
                    fontWeight: FontWeight.w700,
                    color: cc.moon,
                  ),
                ),
                const SizedBox(height: 10),
                ConstrainedBox(
                  constraints: const BoxConstraints(maxHeight: 180),
                  child: SingleChildScrollView(
                    child: SelectableText(
                      text,
                      style: TextStyle(fontSize: 14, color: cc.moon),
                    ),
                  ),
                ),
                const SizedBox(height: 16),
                Row(
                  mainAxisAlignment: MainAxisAlignment.end,
                  children: [
                    TextButton.icon(
                      onPressed: () {
                        Navigator.pop(sheetContext);
                        _insertDroppedText(text);
                      },
                      icon: const Icon(Icons.edit_rounded),
                      label: const Text('填入输入框'),
                    ),
                    const SizedBox(width: 8),
                    FilledButton.icon(
                      onPressed: () {
                        Navigator.pop(sheetContext);
                        _input.text = text;
                        _sendText();
                      },
                      icon: const Icon(Icons.send_rounded),
                      label: const Text('直接发送'),
                    ),
                  ],
                ),
              ],
            ),
          ),
        );
      },
    );
  }

  Future<void> _onAccept(TransferItem t) async {
    final state = context.read<AppState>();
    // 设置页配置了默认保存目录则直接用；否则让用户选择
    String dir;
    final configured = state.defaultDownloadDir;
    if (configured != null && configured.isNotEmpty) {
      dir = configured;
    } else {
      final picked = await getDirectoryPath();
      if (picked == null) return;
      dir = picked;
    }
    final err = await state.acceptTransfer(t.transferId, dir);
    if (err != null && mounted) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(err)));
    }
  }

  Future<void> _openTransferFile(TransferItem transfer) async {
    final error = await PlatformFiles.openFile(transfer.localPath);
    if (error != null && mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(error)));
    }
  }

  Future<void> _openTransferFolder(TransferItem transfer) async {
    final error = await PlatformFiles.openContainingFolder(transfer.localPath);
    if (error != null && mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(error)));
    }
  }

  void _previewTransfer(TransferItem transfer) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => MediaPreviewPage(transfer: transfer),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    final state = context.watch<AppState>();
    final peer = state.peer(widget.deviceId);
    final name = state.peerName(widget.deviceId);
    final online = peer?.online ?? false;
    final trusted = state.isTrusted(widget.deviceId);
    final messages = state.messagesOf(widget.deviceId);
    final transfers = state.transfersOf(widget.deviceId);
    final timeline =
        <_TimelineEntry>[
          for (var i = 0; i < messages.length; i++)
            _TimelineEntry.message(messages[i], i),
          for (var i = 0; i < transfers.length; i++)
            _TimelineEntry.transfer(transfers[i], messages.length + i),
        ]..sort((a, b) {
          final byTime = a.tsMs.compareTo(b.tsMs);
          return byTime != 0 ? byTime : a.stableOrder.compareTo(b.stableOrder);
        });

    if (_lastTimelineLength != timeline.length) {
      _lastTimelineLength = timeline.length;
      _scrollToBottom();
    }

    return Container(
      color: cc.night, // 自带背景：push 路由与内嵌模式都避免黑屏
      child: Column(
        children: [
          _header(context, name, online, trusted),
          Expanded(
            child: DropTarget(
              onDragEntered: (_) => setState(() => _dragging = true),
              onDragExited: (_) => setState(() => _dragging = false),
              onDragDone: (details) async {
                setState(() => _dragging = false);
                final droppedText = details.text;
                if (droppedText != null && droppedText.trim().isNotEmpty) {
                  _insertDroppedText(droppedText);
                }
                for (final f in details.files) {
                  await _sendPath(f.path);
                }
              },
              child: Stack(
                children: [
                  messages.isEmpty && transfers.isEmpty
                      ? Center(
                          child: Column(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              Icon(
                                Icons.nightlight_round,
                                size: 46,
                                color: cc.nightSoft,
                              ),
                              SizedBox(height: 12),
                              Text(
                                '还没有消息',
                                style: TextStyle(color: cc.moonDim),
                              ),
                              SizedBox(height: 4),
                              Text(
                                '发送文字、链接，或把文件拖进来',
                                style: TextStyle(
                                  fontSize: 12,
                                  color: cc.moonDim,
                                ),
                              ),
                            ],
                          ),
                        )
                      : ListView.builder(
                          controller: _scroll,
                          padding: const EdgeInsets.symmetric(vertical: 10),
                          itemCount: timeline.length,
                          itemBuilder: (context, i) {
                            final entry = timeline[i];
                            final showDate =
                                i == 0 ||
                                !isSameLocalDay(
                                  timeline[i - 1].tsMs,
                                  entry.tsMs,
                                );
                            final content = entry.message != null
                                ? MessageBubble(
                                    message: entry.message!,
                                    peerName: name,
                                  )
                                : _transferBubble(state, entry.transfer!);
                            final id =
                                entry.message?.id ?? entry.transfer!.transferId;
                            return TimelineEntrance(
                              key: ValueKey(id),
                              child: Column(
                                children: [
                                  if (showDate) _dateSeparator(entry.tsMs),
                                  content,
                                ],
                              ),
                            );
                          },
                        ),
                  if (_dragging)
                    Container(
                      decoration: BoxDecoration(
                        color: const Color(0x33E8C87A),
                        border: Border.all(color: cc.gold, width: 2),
                        borderRadius: BorderRadius.circular(20),
                      ),
                      margin: const EdgeInsets.all(14),
                      child: Center(
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Icon(
                              Icons.file_download_done_rounded,
                              size: 40,
                              color: cc.gold,
                            ),
                            SizedBox(height: 8),
                            Text(
                              '松开以发送文件或填入文字',
                              style: TextStyle(
                                color: cc.gold,
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                          ],
                        ),
                      ),
                    ),
                ],
              ),
            ),
          ),
          _inputBar(context),
        ],
      ),
    );
  }

  Widget _dateSeparator(int tsMs) {
    final cc = LunoteColors.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: cc.nightSoft,
          borderRadius: BorderRadius.circular(12),
        ),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
          child: Text(
            formatTimelineDate(tsMs),
            style: TextStyle(fontSize: 10.5, color: cc.moonDim),
          ),
        ),
      ),
    );
  }

  Widget _transferBubble(AppState state, TransferItem transfer) {
    final canPreview =
        transfer.isDone &&
        transfer.localPath != null &&
        isPreviewableImage(transfer.fileName) &&
        File(transfer.localPath!).existsSync();
    return TransferTile(
      transfer: transfer,
      compact: true,
      onPreview: canPreview ? () => _previewTransfer(transfer) : null,
      onOpen: transfer.localPath == null
          ? null
          : () => _openTransferFile(transfer),
      onOpenFolder: transfer.localPath == null
          ? null
          : () => _openTransferFolder(transfer),
      onAccept: () => _onAccept(transfer),
      onReject: () async {
        final error = await state.rejectTransfer(transfer.transferId, '用户拒绝');
        if (error != null && mounted) {
          ScaffoldMessenger.of(context)
              .showSnackBar(SnackBar(content: Text(error)));
        }
      },
      onCancel: () async {
        final error = await state.cancelTransfer(transfer.transferId);
        if (error != null && mounted) {
          ScaffoldMessenger.of(context)
              .showSnackBar(SnackBar(content: Text(error)));
        }
      },
      onRetry: () async {
        final path = state.sentPaths[transfer.transferId] ?? transfer.localPath;
        if (path == null) {
          if (mounted) {
            ScaffoldMessenger.of(context).showSnackBar(
              const SnackBar(content: Text('无法自动重试：原文件路径未知，请重新选择发送')),
            );
          }
          return;
        }
        final error = await state.sendFile(widget.deviceId, path);
        if (error != null && mounted) {
          ScaffoldMessenger.of(context)
              .showSnackBar(SnackBar(content: Text(error)));
        }
      },
    );
  }

  Widget _header(BuildContext context, String name, bool online, bool trusted) {
    final cc = LunoteColors.of(context);
    final canPop = Navigator.of(context).canPop();
    final showBack = canPop || widget.onBack != null;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
      decoration: BoxDecoration(
        color: cc.nightRaised,
        border: Border(bottom: BorderSide(color: cc.nightSoft, width: 1)),
      ),
      child: Row(
        children: [
          if (showBack)
            SpringButton(
              weight: SpringWeight.icon,
              onTap: () {
                if (canPop) {
                  Navigator.of(context).pop();
                } else {
                  widget.onBack?.call();
                }
              },
              child: Padding(
                padding: EdgeInsets.all(6),
                child: Icon(
                  Icons.arrow_back_rounded,
                  size: 20,
                  color: cc.moonDim,
                ),
              ),
            ),
          Container(
            width: 34,
            height: 34,
            decoration: BoxDecoration(
              color: cc.nightSoft,
              shape: BoxShape.circle,
            ),
            alignment: Alignment.center,
            child: Text(
              name.isEmpty
                  ? '?'
                  : String.fromCharCode(name.runes.first).toUpperCase(),
              style: TextStyle(color: cc.gold, fontWeight: FontWeight.w700),
            ),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  name,
                  style: TextStyle(
                    fontSize: 15,
                    fontWeight: FontWeight.w700,
                    color: cc.moon,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  online ? '在线' : '离线',
                  style: TextStyle(
                    fontSize: 11.5,
                    color: online ? cc.online : cc.offline,
                  ),
                ),
              ],
            ),
          ),
          if (trusted)
            Row(
              children: [
                Icon(Icons.verified_rounded, size: 16, color: cc.online),
                SizedBox(width: 4),
                Text('可信设备', style: TextStyle(fontSize: 12, color: cc.online)),
              ],
            ),
        ],
      ),
    );
  }

  Widget _inputBar(BuildContext context) {
    final cc = LunoteColors.of(context);
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 10, 14, 14),
      decoration: BoxDecoration(
        color: cc.nightRaised,
        border: Border(top: BorderSide(color: cc.nightSoft, width: 1)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          SpringButton(
            weight: SpringWeight.icon,
            onTap: _pickFiles,
            child: _iconBtn(Icons.attach_file_rounded, '文件'),
          ),
          const SizedBox(width: 6),
          SpringButton(
            weight: SpringWeight.icon,
            onTap: _pickFolder,
            child: _iconBtn(Icons.folder_rounded, '文件夹'),
          ),
          const SizedBox(width: 6),
          SpringButton(
            weight: SpringWeight.icon,
            onTap: _showClipboard,
            child: _iconBtn(Icons.content_paste_go_rounded, '读取系统剪贴板'),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 4),
              decoration: BoxDecoration(
                color: cc.night,
                borderRadius: BorderRadius.circular(22),
                border: Border.all(color: cc.nightSoft),
              ),
              child: TextField(
                controller: _input,
                focusNode: _inputFocus,
                maxLines: 1,
                minLines: 1,
                style: TextStyle(fontSize: 14, color: cc.moon),
                decoration: InputDecoration(
                  border: InputBorder.none,
                  hintText: '输入消息…（自动识别链接）',
                  hintStyle: TextStyle(color: cc.moonDim, fontSize: 13.5),
                  isDense: true,
                ),
                onSubmitted: (_) => _sendText(),
              ),
            ),
          ),
          const SizedBox(width: 10),
          SpringButton(
            weight: SpringWeight.primary,
            onTap: _sendText,
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(color: cc.gold, shape: BoxShape.circle),
              child: Icon(Icons.send_rounded, size: 18, color: cc.night),
            ),
          ),
        ],
      ),
    );
  }

  Widget _iconBtn(IconData icon, String tooltip) {
    final cc = LunoteColors.of(context);
    return Tooltip(
      message: tooltip,
      child: Container(
        width: 38,
        height: 38,
        decoration: BoxDecoration(
          color: cc.nightSoft,
          borderRadius: BorderRadius.circular(12),
        ),
        child: Icon(icon, size: 19, color: cc.moonDim),
      ),
    );
  }
}

class _TimelineEntry {
  const _TimelineEntry._({
    required this.tsMs,
    required this.stableOrder,
    this.message,
    this.transfer,
  });

  factory _TimelineEntry.message(MessageItem message, int stableOrder) =>
      _TimelineEntry._(
        tsMs: message.tsMs,
        stableOrder: stableOrder,
        message: message,
      );

  factory _TimelineEntry.transfer(TransferItem transfer, int stableOrder) =>
      _TimelineEntry._(
        tsMs: transfer.tsMs,
        stableOrder: stableOrder,
        transfer: transfer,
      );

  final int tsMs;
  final int stableOrder;
  final MessageItem? message;
  final TransferItem? transfer;
}
