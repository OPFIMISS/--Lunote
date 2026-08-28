import 'package:flutter/material.dart';

String formatClock(int tsMs) {
  final time = DateTime.fromMillisecondsSinceEpoch(tsMs).toLocal();
  final hour = time.hour.toString().padLeft(2, '0');
  final minute = time.minute.toString().padLeft(2, '0');
  return '$hour:$minute';
}

String formatTimelineDate(int tsMs) {
  final date = DateTime.fromMillisecondsSinceEpoch(tsMs).toLocal();
  final now = DateTime.now();
  final today = DateUtils.dateOnly(now);
  final value = DateUtils.dateOnly(date);
  final delta = today.difference(value).inDays;
  if (delta == 0) return '今天';
  if (delta == 1) return '昨天';
  if (date.year == now.year) return '${date.month}月${date.day}日';
  return '${date.year}年${date.month}月${date.day}日';
}

bool isSameLocalDay(int firstMs, int secondMs) {
  final a = DateTime.fromMillisecondsSinceEpoch(firstMs).toLocal();
  final b = DateTime.fromMillisecondsSinceEpoch(secondMs).toLocal();
  return a.year == b.year && a.month == b.month && a.day == b.day;
}
