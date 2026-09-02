import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../core/models.dart';
import '../file_types.dart';
import '../formatters.dart';
import '../lunote_theme.dart';
import 'image_thumbnail.dart';
import 'spring_button.dart';

/// 文件气泡：聊天页与传输中心共用同一状态和操作。
class TransferTile extends StatelessWidget {
  const TransferTile({
    super.key,
    required this.transfer,
    required this.onAccept,
    required this.onReject,
    required this.onCancel,
    required this.onRetry,
    this.onPause,
    this.onResume,
    this.onOpen,
    this.onOpenFolder,
    this.onPreview,
    this.onLongPress,
    this.selected = false,
    this.imagePreviewEnabled = true,
    this.compact = false,
  });

  final TransferItem transfer;
  final VoidCallback onAccept;
  final VoidCallback onReject;
  final VoidCallback onCancel;
  final VoidCallback onRetry;
  final VoidCallback? onPause;
  final VoidCallback? onResume;
  final VoidCallback? onOpen;
  final VoidCallback? onOpenFolder;
  final VoidCallback? onPreview;
  final VoidCallback? onLongPress;
  final bool selected;
  final bool compact;
  final bool imagePreviewEnabled;

  String get _stateLabel {
    switch (transfer.state) {
      case 'offered':
        return transfer.isOutgoing ? '等待对方确认' : '等待你确认';
      case 'accepted':
        return '已接受';
      case 'in_progress':
        return '传输中';
      case 'paused':
        return '已暂停';
      case 'done':
        return '已完成 · 校验通过';
      case 'failed':
        return '失败（可续传）';
      case 'canceled':
        return '已取消';
      case 'rejected':
        return '已拒绝';
      default:
        return transfer.state;
    }
  }

  Color _stateColor(LunoteColors cc) {
    switch (transfer.state) {
      case 'done':
        return cc.online;
      case 'failed':
      case 'rejected':
        return cc.warn;
      case 'canceled':
        return cc.offline;
      default:
        return cc.gold;
    }
  }

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    final t = transfer;
    final category = categoryForFile(t.fileName);
    final canPreview =
        t.isDone &&
        t.localPath != null &&
        imagePreviewEnabled &&
        isPreviewableImage(t.fileName) &&
        File(t.localPath!).existsSync();
    final screenWidth = MediaQuery.sizeOf(context).width;
    final compactWidth = math.min(
      screenWidth * (screenWidth < 700 ? 0.86 : 0.68),
      620.0,
    );

    final bubble = AnimatedContainer(
      duration: const Duration(milliseconds: 220),
      curve: Curves.easeOutCubic,
      width: compact ? compactWidth : double.infinity,
      margin: EdgeInsets.symmetric(horizontal: 18, vertical: compact ? 5 : 6),
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: cc.nightRaised,
        borderRadius: BorderRadius.only(
          topLeft: Radius.circular(compact && !t.isOutgoing ? 6 : 16),
          topRight: Radius.circular(compact && t.isOutgoing ? 6 : 16),
          bottomLeft: const Radius.circular(16),
          bottomRight: const Radius.circular(16),
        ),
        border: cc.isGlass ? Border.all(color: const Color(0xB8FFFFFF)) : null,
        boxShadow: [
          BoxShadow(
            color: cc.isGlass
                ? const Color(0x24384A61)
                : const Color(0x16000000),
            blurRadius: cc.isGlass ? 18 : 7,
            offset: const Offset(0, 4),
          ),
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Container(
                width: 38,
                height: 38,
                decoration: BoxDecoration(
                  color: t.isOutgoing ? cc.gold : cc.linkBlue,
                  borderRadius: BorderRadius.circular(12),
                ),
                child: Icon(
                  category.icon,
                  size: 20,
                  color: cc.isGlass || t.isOutgoing ? Colors.white : cc.night,
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      t.fileName,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 13.5,
                        fontWeight: FontWeight.w700,
                        color: cc.moon,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      '${t.humanSize} · ${t.isOutgoing ? '发送' : '接收'}',
                      style: TextStyle(fontSize: 10.5, color: cc.moonDim),
                    ),
                  ],
                ),
              ),
            ],
          ),
          if (canPreview) ...[
            const SizedBox(height: 10),
            Hero(
              tag: 'transfer-media-${t.transferId}',
              child: ImageThumbnail(path: t.localPath!, onTap: onPreview),
            ),
          ],
          if (t.isInProgress || t.isPaused || t.state == 'done') ...[
            const SizedBox(height: 10),
            _TransferProgress(transfer: t, colors: cc),
          ],
          const SizedBox(height: 7),
          Row(
            children: [
              Expanded(
                child: AnimatedSwitcher(
                  duration: const Duration(milliseconds: 180),
                  child: Text(
                    _stateLabel,
                    key: ValueKey('${t.state}-${t.direction}'),
                    style: TextStyle(
                      fontSize: 11.5,
                      fontWeight: FontWeight.w600,
                      color: _stateColor(cc),
                    ),
                  ),
                ),
              ),
              const SizedBox(width: 8),
              Text(
                formatClock(t.tsMs),
                style: TextStyle(fontSize: 9.5, color: cc.moonDim),
              ),
            ],
          ),
          if (t.error != null)
            Padding(
              padding: const EdgeInsets.only(top: 5),
              child: Text(
                t.error!,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 11, color: cc.warn),
              ),
            ),
          if (_hasActions) ...[
            const SizedBox(height: 11),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              alignment: WrapAlignment.end,
              children: _actions(cc),
            ),
          ],
        ],
      ),
    );

    final wrapped = GestureDetector(
      onLongPress: onLongPress,
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          bubble,
          if (selected)
            const Positioned(
              top: -5,
              right: -5,
              child: Icon(
                Icons.check_circle_rounded,
                size: 22,
                color: Colors.amber,
              ),
            ),
        ],
      ),
    );
    if (!compact) return wrapped;
    return Align(
      alignment: t.isOutgoing ? Alignment.centerRight : Alignment.centerLeft,
      child: wrapped,
    );
  }

  bool get _hasActions =>
      transfer.isOffered ||
      transfer.isInProgress ||
      transfer.isPaused ||
      (transfer.isFailed && transfer.isOutgoing) ||
      (transfer.isDone && (onOpen != null || onOpenFolder != null));

  List<Widget> _actions(LunoteColors cc) {
    final actions = <Widget>[];
    if (transfer.isDone && onOpen != null) {
      actions.add(
        _button(cc, Icons.open_in_new_rounded, '打开文件', onOpen!, primary: true),
      );
    }
    if (transfer.isDone && onOpenFolder != null) {
      actions.add(
        _button(cc, Icons.folder_open_rounded, '所在文件夹', onOpenFolder!),
      );
    }
    if (transfer.isOffered && !transfer.isOutgoing) {
      actions
        ..add(_button(cc, Icons.close_rounded, '拒绝', onReject))
        ..add(
          _button(
            cc,
            Icons.download_done_rounded,
            '接收',
            onAccept,
            primary: true,
          ),
        );
    }
    if (transfer.isOffered && transfer.isOutgoing) {
      actions.add(_button(cc, Icons.close_rounded, '取消发送', onCancel));
    }
    if (transfer.isInProgress) {
      if (onPause != null) {
        actions.add(_button(cc, Icons.pause_rounded, '暂停', onPause!));
      }
      actions.add(_button(cc, Icons.stop_rounded, '取消', onCancel));
    }
    if (transfer.isPaused && onResume != null) {
      actions.add(
        _button(cc, Icons.play_arrow_rounded, '继续', onResume!, primary: true),
      );
      actions.add(_button(cc, Icons.stop_rounded, '取消', onCancel));
    }
    if (transfer.isFailed && transfer.isOutgoing) {
      actions.add(
        _button(cc, Icons.refresh_rounded, '重新发送', onRetry, primary: true),
      );
    }
    return actions;
  }

  Widget _button(
    LunoteColors cc,
    IconData icon,
    String label,
    VoidCallback onTap, {
    bool primary = false,
  }) {
    return SpringButton(
      weight: primary ? SpringWeight.primary : SpringWeight.normal,
      onTap: onTap,
      child: Container(
        height: 36,
        padding: const EdgeInsets.symmetric(horizontal: 12),
        decoration: BoxDecoration(
          color: primary ? cc.gold : cc.nightSoft,
          borderRadius: BorderRadius.circular(12),
          border: cc.isGlass
              ? Border.all(color: const Color(0x99FFFFFF))
              : null,
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 15, color: primary ? Colors.white : cc.moon),
            const SizedBox(width: 5),
            Text(
              label,
              style: TextStyle(
                fontSize: 11.5,
                fontWeight: FontWeight.w700,
                color: primary ? Colors.white : cc.moon,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _TransferProgress extends StatelessWidget {
  const _TransferProgress({required this.transfer, required this.colors});

  final TransferItem transfer;
  final LunoteColors colors;

  String get _remaining {
    if (transfer.speedBps <= 0) return '正在测速';
    final bytes = math.max(0, transfer.fileSize - transfer.transferred);
    final seconds = (bytes / transfer.speedBps).ceil();
    if (seconds < 60) return '预计剩余 ${seconds}s';
    final minutes = seconds ~/ 60;
    final rest = seconds % 60;
    if (minutes < 60) return '预计剩余 ${minutes}m ${rest}s';
    return '预计剩余 ${minutes ~/ 60}h ${minutes % 60}m';
  }

  @override
  Widget build(BuildContext context) {
    final target = transfer.isDone ? 1.0 : transfer.progress;
    final reduceMotion = MediaQuery.disableAnimationsOf(context);
    return TweenAnimationBuilder<double>(
      tween: Tween<double>(end: target),
      duration: reduceMotion
          ? Duration.zero
          : const Duration(milliseconds: 320),
      curve: Curves.easeOutCubic,
      builder: (context, progress, _) => Column(
        children: [
          ClipRRect(
            borderRadius: BorderRadius.circular(5),
            child: LinearProgressIndicator(
              value: progress,
              minHeight: 6,
              backgroundColor: colors.nightSoft,
              color: transfer.isDone ? colors.online : colors.gold,
            ),
          ),
          if (transfer.isInProgress) ...[
            const SizedBox(height: 7),
            Row(
              children: [
                Expanded(
                  child: Text(
                    transfer.speedBps > 0
                        ? '传输速度 ${transfer.humanSpeed} · $_remaining'
                        : _remaining,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 10.5, color: colors.moonDim),
                  ),
                ),
                const SizedBox(width: 8),
                Text(
                  '${(progress * 100).toStringAsFixed(0)}%',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                    color: colors.moonDim,
                  ),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}
