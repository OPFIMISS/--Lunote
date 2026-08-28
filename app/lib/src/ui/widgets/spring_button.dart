import 'package:flutter/material.dart';
import 'package:flutter/physics.dart';

import '../lunote_theme.dart';

/// 柔软的按压按钮：按下轻微挤压，松手弹簧回弹（带过冲后回稳）。
/// 不同 weight 使用不同弹簧参数，避免所有组件机械地执行同一段动画。
class SpringButton extends StatefulWidget {
  const SpringButton({
    super.key,
    required this.child,
    required this.onTap,
    this.weight = SpringWeight.normal,
    this.onLongPress,
    this.borderRadius = 12,
    this.enabled = true,
  });

  final Widget child;
  final VoidCallback? onTap;
  final VoidCallback? onLongPress;
  final SpringWeight weight;
  final double borderRadius;
  final bool enabled;

  @override
  State<SpringButton> createState() => _SpringButtonState();
}

enum SpringWeight { primary, normal, icon, panel }

class _SpringButtonState extends State<SpringButton>
    with SingleTickerProviderStateMixin {
  late final AnimationController _ctrl;
  late final Animation<double> _scale;

  @override
  void initState() {
    super.initState();
    _ctrl = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 320),
    );
    _scale = _ctrl.drive(Tween(begin: 1.0, end: 0.94));
  }

  SpringDescription get _spring {
    switch (widget.weight) {
      case SpringWeight.primary:
        return LunoteSprings.primary;
      case SpringWeight.icon:
        return LunoteSprings.icon;
      case SpringWeight.panel:
        return LunoteSprings.panel;
      case SpringWeight.normal:
        return LunoteSprings.normal;
    }
  }

  void _press() {
    if (!widget.enabled) return;
    _ctrl.animateTo(
      1,
      duration: const Duration(milliseconds: 90),
      curve: Curves.easeOut,
    );
  }

  void _release() {
    if (!widget.enabled) return;
    // 弹簧回弹：到达目标后过冲再回摆
    _ctrl.animateWith(
      SpringSimulation(_spring, _ctrl.value, 0, _ctrl.velocity),
    );
  }

  @override
  void dispose() {
    _ctrl.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTapDown: (_) => _press(),
      onTapUp: (_) => _release(),
      onTapCancel: _release,
      onTap: widget.enabled ? widget.onTap : null,
      onLongPress: widget.enabled ? widget.onLongPress : null,
      child: AnimatedBuilder(
        animation: _scale,
        builder: (context, child) {
          return Transform.scale(scale: _scale.value, child: child);
        },
        child: widget.child,
      ),
    );
  }
}
