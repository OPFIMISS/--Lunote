import 'dart:io';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../../core/models.dart';
import '../../core/platform_files.dart';
import '../../state/app_state.dart';
import '../lunote_theme.dart';

class MediaPreviewPage extends StatelessWidget {
  const MediaPreviewPage({super.key, required this.transfer});

  final TransferItem transfer;

  Future<void> _run(BuildContext context, Future<String?> action) async {
    final error = await action;
    if (error != null && context.mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(error)));
    }
  }

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    final state = context.read<AppState>();
    final path = transfer.localPath!;
    final treeUri = (!transfer.isOutgoing && state.receiveTreeUri != null)
        ? state.receiveTreeUri
        : null;
    return Scaffold(
      backgroundColor: const Color(0xF2141822),
      appBar: AppBar(
        backgroundColor: Colors.transparent,
        foregroundColor: Colors.white,
        elevation: 0,
        title: Text(transfer.fileName, overflow: TextOverflow.ellipsis),
        actions: [
          IconButton(
            tooltip: '打开文件',
            onPressed: () => _run(context, PlatformFiles.openFile(path)),
            icon: const Icon(Icons.open_in_new_rounded),
          ),
          IconButton(
            tooltip: '打开所在文件夹',
            onPressed: () => _run(
              context,
              PlatformFiles.openContainingFolder(path, treeUri: treeUri),
            ),
            icon: const Icon(Icons.folder_open_rounded),
          ),
          const SizedBox(width: 6),
        ],
      ),
      body: Column(
        children: [
          Expanded(
            child: Hero(
              tag: 'transfer-media-${transfer.transferId}',
              child: InteractiveViewer(
                minScale: 0.7,
                maxScale: 5,
                child: Center(
                  child: Image.file(
                    File(path),
                    fit: BoxFit.contain,
                    errorBuilder: (_, error, _) => Column(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Icon(
                          Icons.broken_image_rounded,
                          size: 52,
                          color: cc.warn,
                        ),
                        const SizedBox(height: 12),
                        const Text(
                          '无法预览该图片',
                          style: TextStyle(color: Colors.white),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
          SafeArea(
            top: false,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(20, 10, 20, 16),
              child: Text(
                '${transfer.humanSize} · 双指缩放',
                style: const TextStyle(fontSize: 12, color: Colors.white70),
              ),
            ),
          ),
        ],
      ),
    );
  }
}
