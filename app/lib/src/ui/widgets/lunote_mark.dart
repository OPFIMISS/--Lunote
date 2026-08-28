import 'package:flutter/material.dart';

import '../lunote_theme.dart';

/// 月牙托住信笺：应用内品牌标记，与桌面及 Android 图标保持一致。
class LunoteMark extends StatelessWidget {
  const LunoteMark({super.key, this.size = 40});

  final double size;

  @override
  Widget build(BuildContext context) {
    final colors = LunoteColors.of(context);
    return SizedBox.square(
      dimension: size,
      child: CustomPaint(
        painter: _LunoteMarkPainter(
          background: colors.isGlass ? const Color(0xFFEAF8F7) : colors.night,
          moon: colors.gold,
        ),
      ),
    );
  }
}

class _LunoteMarkPainter extends CustomPainter {
  const _LunoteMarkPainter({required this.background, required this.moon});

  final Color background;
  final Color moon;

  @override
  void paint(Canvas canvas, Size size) {
    final scale = size.width / 64;
    final radius = Radius.circular(15 * scale);
    canvas.drawRRect(
      RRect.fromRectAndRadius(Offset.zero & size, radius),
      Paint()..color = background,
    );

    canvas.save();
    canvas.scale(scale);
    canvas.drawCircle(const Offset(27, 33), 19, Paint()..color = moon);
    canvas.drawCircle(const Offset(35, 25), 15.5, Paint()..color = background);

    final paper = Path()
      ..moveTo(28, 18)
      ..lineTo(43, 18)
      ..lineTo(51, 26)
      ..lineTo(51, 47)
      ..quadraticBezierTo(51, 50, 48, 50)
      ..lineTo(28, 50)
      ..quadraticBezierTo(25, 50, 25, 47)
      ..lineTo(25, 21)
      ..quadraticBezierTo(25, 18, 28, 18)
      ..close();
    canvas.drawPath(paper, Paint()..color = Colors.white);

    final fold = Path()
      ..moveTo(43, 18)
      ..lineTo(43, 26)
      ..lineTo(51, 26)
      ..close();
    canvas.drawPath(fold, Paint()..color = const Color(0xFFD8EEF0));

    final ink = Paint()
      ..color = const Color(0xFF3E7080)
      ..strokeWidth = 2.2
      ..strokeCap = StrokeCap.round;
    canvas.drawLine(const Offset(31, 34), const Offset(45, 34), ink);
    canvas.drawLine(const Offset(31, 40), const Offset(42, 40), ink);
    canvas.restore();
  }

  @override
  bool shouldRepaint(covariant _LunoteMarkPainter oldDelegate) =>
      oldDelegate.background != background || oldDelegate.moon != moon;
}
