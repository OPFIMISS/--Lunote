import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';

import '../lunote_theme.dart';

/// 只解码第一帧，避免 GIF 在对话列表中持续播放，也限制大图解码尺寸。
class ImageThumbnail extends StatefulWidget {
  const ImageThumbnail({super.key, required this.path, this.onTap});

  final String path;
  final VoidCallback? onTap;

  @override
  State<ImageThumbnail> createState() => _ImageThumbnailState();
}

class _ImageThumbnailState extends State<ImageThumbnail> {
  ui.Image? _image;
  Object? _error;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void didUpdateWidget(covariant ImageThumbnail oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.path != widget.path) {
      _image?.dispose();
      _image = null;
      _error = null;
      _load();
    }
  }

  Future<void> _load() async {
    try {
      final bytes = await File(widget.path).readAsBytes();
      final codec = await ui.instantiateImageCodec(bytes, targetWidth: 720);
      final frame = await codec.getNextFrame();
      codec.dispose();
      if (!mounted) {
        frame.image.dispose();
        return;
      }
      setState(() => _image = frame.image);
    } catch (error) {
      if (mounted) setState(() => _error = error);
    }
  }

  @override
  void dispose() {
    _image?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    final image = _image;
    final aspect = image == null || image.height == 0
        ? 16 / 9
        : image.width / image.height;
    return Semantics(
      button: widget.onTap != null,
      label: '预览图片',
      child: InkWell(
        onTap: widget.onTap,
        borderRadius: BorderRadius.circular(10),
        child: ClipRRect(
          borderRadius: BorderRadius.circular(10),
          child: LayoutBuilder(
            builder: (context, constraints) {
              final width = constraints.maxWidth.isFinite
                  ? constraints.maxWidth
                  : 360.0;
              final height = (width / aspect).clamp(120.0, 420.0);
              return SizedBox(
                width: width,
                height: height,
                child: ColoredBox(
                  color: cc.nightSoft,
                  child: image != null
                      ? RawImage(image: image, fit: BoxFit.contain)
                      : Center(
                      child: _error == null
                          ? SizedBox(
                              width: 20,
                              height: 20,
                              child: CircularProgressIndicator(
                                strokeWidth: 2,
                                color: cc.gold,
                              ),
                            )
                          : Icon(Icons.broken_image_rounded, color: cc.moonDim),
                      ),
                ),
              );
            },
          ),
        ),
      ),
    );
  }
}
