import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../../core/models.dart';
import '../../core/platform_files.dart';
import '../../state/app_state.dart';
import '../file_types.dart';
import '../lunote_theme.dart';
import '../widgets/timeline_entrance.dart';
import '../widgets/transfer_tile.dart';
import 'media_preview_page.dart';

/// 传输中心：全部传输的真实状态（进度/取消/接受/拒绝/重试）。
class TransfersPage extends StatefulWidget {
  const TransfersPage({super.key});

  @override
  State<TransfersPage> createState() => _TransfersPageState();
}

class _TransfersPageState extends State<TransfersPage> {
  TransferCategory _category = TransferCategory.all;
  String _status = 'all';

  void _showError(BuildContext context, String? error) {
    if (error != null && context.mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(error)));
    }
  }

  Future<void> _openFile(BuildContext context, TransferItem transfer) async {
    final error = await PlatformFiles.openFile(transfer.localPath);
    if (context.mounted) _showError(context, error);
  }

  Future<void> _openFolder(BuildContext context, TransferItem transfer) async {
    final error = await PlatformFiles.openContainingFolder(transfer.localPath);
    if (context.mounted) _showError(context, error);
  }

  void _preview(BuildContext context, TransferItem transfer) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => MediaPreviewPage(transfer: transfer),
      ),
    );
  }

  bool _matchesStatus(TransferItem transfer) => switch (_status) {
    'active' => transfer.isOffered || transfer.isInProgress || transfer.isPaused,
    'done' => transfer.isDone,
    'failed' =>
      transfer.isFailed || transfer.isCanceled || transfer.state == 'rejected',
    _ => true,
  };

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    final state = context.watch<AppState>();
    final all = state.allTransfers.reversed.toList();
    final list = all.where((transfer) {
      final category = categoryForFile(transfer.fileName);
      return (_category == TransferCategory.all || _category == category) &&
          _matchesStatus(transfer);
    }).toList();
    final active = list.where((t) => t.isInProgress || t.isPaused).toList();
    final totalBytes = active.fold<int>(0, (sum, t) => sum + t.fileSize);
    final doneBytes = active.fold<int>(0, (sum, t) => sum + t.transferred);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 20, 24, 4),
          child: Text(
            '传输',
            style: TextStyle(
              fontSize: 22,
              fontWeight: FontWeight.w700,
              color: cc.moon,
            ),
          ),
        ),
        if (active.length > 1)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 0, 24, 8),
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
              decoration: BoxDecoration(color: cc.nightRaised, borderRadius: BorderRadius.circular(10), border: Border.all(color: cc.nightSoft)),
              child: Row(
                children: [
                  Expanded(child: Text('任务组 · ${active.length} 个文件 · ${_size(totalBytes)} · ${totalBytes == 0 ? 0 : (doneBytes * 100 / totalBytes).round()}%', style: TextStyle(fontSize: 11.5, color: cc.moon))),
                  IconButton(tooltip: '暂停全部', onPressed: () async { final e = await state.pauseTransfers(active.where((t) => t.isInProgress).map((t) => t.transferId)); if (e != null && context.mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(e))); }, icon: const Icon(Icons.pause_circle_outline_rounded, size: 20)),
                  IconButton(tooltip: '继续全部', onPressed: () async { final e = await state.resumeTransfers(active.where((t) => t.isPaused).map((t) => t.transferId)); if (e != null && context.mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(e))); }, icon: const Icon(Icons.play_circle_outline_rounded, size: 20)),
                  IconButton(tooltip: '取消全部', onPressed: () async { final e = await state.cancelTransfers(active.map((t) => t.transferId)); if (e != null && context.mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(e))); }, icon: Icon(Icons.stop_circle_outlined, size: 20, color: cc.warn)),
                ],
              ),
            ),
          ),
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 4, 24, 12),
          child: Text(
            '文件流式传输 · 完整性校验 · 断点续传（状态来自核心事件）',
            style: TextStyle(fontSize: 12.5, color: cc.moonDim),
          ),
        ),
        SizedBox(
          height: 42,
          child: ListView(
            scrollDirection: Axis.horizontal,
            padding: const EdgeInsets.symmetric(horizontal: 18),
            children: [
              for (final category in TransferCategory.values)
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 3),
                  child: FilterChip(
                    avatar: Icon(category.icon, size: 16),
                    label: Text(
                      '${category.label} ${category == TransferCategory.all ? all.length : all.where((item) => categoryForFile(item.fileName) == category).length}',
                    ),
                    selected: _category == category,
                    onSelected: (_) => setState(() => _category = category),
                  ),
                ),
            ],
          ),
        ),
        SizedBox(
          height: 40,
          child: ListView(
            scrollDirection: Axis.horizontal,
            padding: const EdgeInsets.symmetric(horizontal: 21),
            children: [
              for (final item in const [
                ('all', '全部状态'),
                ('active', '进行中'),
                ('done', '已完成'),
                ('failed', '未完成'),
              ])
                Padding(
                  padding: const EdgeInsets.only(right: 8),
                  child: ChoiceChip(
                    label: Text(item.$2),
                    selected: _status == item.$1,
                    onSelected: (_) => setState(() => _status = item.$1),
                  ),
                ),
            ],
          ),
        ),
        Expanded(
          child: list.isEmpty
              ? Center(
                  child: Text(
                    all.isEmpty ? '暂无传输' : '没有符合筛选条件的传输',
                    style: TextStyle(color: cc.moonDim),
                  ),
                )
              : ListView.builder(
                  padding: const EdgeInsets.only(bottom: 18),
                  itemCount: list.length,
                  itemBuilder: (context, i) {
                    final cc = LunoteColors.of(context);
                    final t = list[i];
                    final peerName = state.peerName(t.peerDeviceId);
                    final canPreview =
                        t.isDone &&
                        t.localPath != null &&
                        isPreviewableImage(t.fileName) &&
                        File(t.localPath!).existsSync();
                    return TimelineEntrance(
                      key: ValueKey(t.transferId),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Padding(
                            padding: const EdgeInsets.fromLTRB(24, 6, 24, 0),
                            child: Text(
                              '${t.isOutgoing ? '发送给' : '来自'} $peerName',
                              style: TextStyle(
                                fontSize: 11.5,
                                color: cc.moonDim,
                              ),
                            ),
                          ),
                          TransferTile(
                            transfer: t,
                            onPreview: canPreview
                                ? () => _preview(context, t)
                                : null,
                            onOpen: t.localPath == null
                                ? null
                                : () => _openFile(context, t),
                            onOpenFolder: t.localPath == null
                                ? null
                                : () => _openFolder(context, t),
                            onAccept: () async {
                              String dir;
                              final configured = state.defaultDownloadDir;
                              if (configured != null && configured.isNotEmpty) {
                                dir = configured;
                              } else {
                                final picked = await getDirectoryPath();
                                if (picked == null) return;
                                dir = picked;
                              }
                              final err = await state.acceptTransfer(
                                t.transferId,
                                dir,
                              );
                              if (err != null && context.mounted) {
                                ScaffoldMessenger.of(context)
                                    .showSnackBar(SnackBar(content: Text(err)));
                              }
                            },
                            onReject: () async {
                              final err = await state.rejectTransfer(
                                t.transferId,
                                '用户拒绝',
                              );
                              if (err != null && context.mounted) {
                                ScaffoldMessenger.of(context)
                                    .showSnackBar(SnackBar(content: Text(err)));
                              }
                            },
                            onCancel: () async {
                              final err = await state.cancelTransfer(
                                t.transferId,
                              );
                              if (err != null && context.mounted) {
                                ScaffoldMessenger.of(context)
                                    .showSnackBar(SnackBar(content: Text(err)));
                              }
                            },
                            onPause: t.isInProgress
                                ? () async {
                                    final err = await state.pauseTransfer(t.transferId);
                                    if (err != null && context.mounted) {
                                      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(err)));
                                    }
                                  }
                                : null,
                            onResume: t.isPaused
                                ? () async {
                                    final err = await state.resumeTransfer(t.transferId);
                                    if (err != null && context.mounted) {
                                      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(err)));
                                    }
                                  }
                                : null,
                            onRetry: () async {
                              final path =
                                  state.sentPaths[t.transferId] ?? t.localPath;
                              if (path == null) {
                                if (context.mounted) {
                                  ScaffoldMessenger.of(context).showSnackBar(
                                    const SnackBar(
                                      content: Text('无法自动重试：原文件路径未知，请重新选择发送'),
                                    ),
                                  );
                                }
                                return;
                              }
                              final err = await state.sendFile(
                                t.peerDeviceId,
                                path,
                              );
                              if (err != null && context.mounted) {
                                ScaffoldMessenger.of(context).showSnackBar(
                                  SnackBar(content: Text('重试失败：$err')),
                                );
                              }
                            },
                          ),
                        ],
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }

  String _size(int bytes) {
    if (bytes >= 1073741824) return '${(bytes / 1073741824).toStringAsFixed(1)} GB';
    if (bytes >= 1048576) return '${(bytes / 1048576).toStringAsFixed(1)} MB';
    if (bytes >= 1024) return '${(bytes / 1024).toStringAsFixed(0)} KB';
    return '$bytes B';
  }
}
