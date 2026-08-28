import 'package:flutter/material.dart';

import '../../state/app_state.dart';
import '../lunote_theme.dart';
import 'spring_button.dart';

/// 首次信任确认：展示设备名与指纹短码，用户确认后返回 true（信任），false/取消。
Future<bool?> showTrustDialog(
  BuildContext context,
  AppState state,
  String deviceId,
) async {
  final name = state.peerName(deviceId);
  final fingerprint = await state.core.call('fingerprint', {
    'device_id': deviceId,
  });
  final fp = fingerprint['fingerprint'] as String? ?? '';
  final short = fp.isEmpty
      ? ''
      : '${fp.substring(0, 8)} … ${fp.substring(fp.length - 8)}';
  if (!context.mounted) return null;
  return showDialog<bool>(
    context: context,
    barrierDismissible: false,
    builder: (ctx) {
      final cc = LunoteColors.of(ctx);
      return Dialog(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Container(
                    width: 42,
                    height: 42,
                    decoration: BoxDecoration(
                      color: cc.gold,
                      shape: BoxShape.circle,
                    ),
                    child: Icon(Icons.auto_awesome, color: cc.night, size: 20),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Text(
                      '信任这台设备？',
                      style: TextStyle(
                        fontSize: 16,
                        fontWeight: FontWeight.w700,
                        color: cc.moon,
                      ),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 16),
              Text(
                '「$name」请求与你建立可信连接。确认后它将成为可信设备，可以向你发送文件。',
                style: TextStyle(
                  fontSize: 13.5,
                  height: 1.5,
                  color: cc.moonDim,
                ),
              ),
              if (short.isNotEmpty) ...[
                const SizedBox(height: 14),
                Container(
                  width: double.infinity,
                  padding: const EdgeInsets.symmetric(
                    horizontal: 14,
                    vertical: 10,
                  ),
                  decoration: BoxDecoration(
                    color: cc.nightSoft,
                    borderRadius: BorderRadius.circular(12),
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        '身份指纹',
                        style: TextStyle(fontSize: 11, color: cc.moonDim),
                      ),
                      const SizedBox(height: 4),
                      Text(
                        short,
                        style: TextStyle(
                          fontSize: 13,
                          fontFamily: 'monospace',
                          color: cc.gold,
                        ),
                      ),
                    ],
                  ),
                ),
              ],
              const SizedBox(height: 20),
              Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  SpringButton(
                    weight: SpringWeight.normal,
                    onTap: () => Navigator.of(ctx).pop(false),
                    child: Padding(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 16,
                        vertical: 9,
                      ),
                      child: Text(
                        '暂不信任',
                        style: TextStyle(color: cc.moonDim, fontSize: 13),
                      ),
                    ),
                  ),
                  const SizedBox(width: 10),
                  SpringButton(
                    weight: SpringWeight.primary,
                    onTap: () => Navigator.of(ctx).pop(true),
                    child: Container(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 22,
                        vertical: 9,
                      ),
                      decoration: BoxDecoration(
                        color: cc.gold,
                        borderRadius: BorderRadius.circular(22),
                      ),
                      child: Text(
                        '确认信任',
                        style: TextStyle(
                          color: cc.night,
                          fontSize: 13,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                    ),
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
