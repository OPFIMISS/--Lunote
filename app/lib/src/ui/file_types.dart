import 'package:flutter/material.dart';

enum TransferCategory { all, image, document, archive, video, audio, other }

extension TransferCategoryUi on TransferCategory {
  String get label => switch (this) {
    TransferCategory.all => '全部',
    TransferCategory.image => '图片',
    TransferCategory.document => '文档',
    TransferCategory.archive => '压缩包',
    TransferCategory.video => '视频',
    TransferCategory.audio => '音频',
    TransferCategory.other => '其他',
  };

  IconData get icon => switch (this) {
    TransferCategory.all => Icons.widgets_rounded,
    TransferCategory.image => Icons.image_rounded,
    TransferCategory.document => Icons.description_rounded,
    TransferCategory.archive => Icons.inventory_2_rounded,
    TransferCategory.video => Icons.movie_rounded,
    TransferCategory.audio => Icons.graphic_eq_rounded,
    TransferCategory.other => Icons.insert_drive_file_rounded,
  };
}

String fileExtension(String fileName) {
  final dot = fileName.lastIndexOf('.');
  return dot < 0 ? '' : fileName.substring(dot + 1).toLowerCase();
}

TransferCategory categoryForFile(String fileName) {
  final ext = fileExtension(fileName);
  if (const {
    'png',
    'jpg',
    'jpeg',
    'webp',
    'gif',
    'bmp',
    'heic',
  }.contains(ext)) {
    return TransferCategory.image;
  }
  if (const {
    'pdf',
    'doc',
    'docx',
    'xls',
    'xlsx',
    'ppt',
    'pptx',
    'txt',
    'md',
    'rtf',
  }.contains(ext)) {
    return TransferCategory.document;
  }
  if (const {'zip', 'rar', '7z', 'tar', 'gz', 'bz2', 'xz'}.contains(ext)) {
    return TransferCategory.archive;
  }
  if (const {'mp4', 'mkv', 'mov', 'avi', 'webm', 'm4v'}.contains(ext)) {
    return TransferCategory.video;
  }
  if (const {'mp3', 'wav', 'flac', 'aac', 'ogg', 'm4a'}.contains(ext)) {
    return TransferCategory.audio;
  }
  return TransferCategory.other;
}

bool isPreviewableImage(String fileName) => const {
  'png',
  'jpg',
  'jpeg',
  'webp',
  'gif',
  'bmp',
}.contains(fileExtension(fileName));
