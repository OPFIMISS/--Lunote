import 'package:flutter/material.dart';
import 'package:flutter/physics.dart';
import 'package:url_launcher/url_launcher_string.dart';

import '../../core/models.dart';
import '../formatters.dart';
import '../lunote_theme.dart';
import '../widgets/spring_button.dart';

/// 消息气泡：新消息以弹簧软着陆；链接可点击打开。
class MessageBubble extends StatefulWidget {
  const MessageBubble({
    super.key,
    required this.message,
    required this.peerName,
    this.imagePreviewEnabled = true,
    this.onLongPress,
    this.selected = false,
  });

  final MessageItem message;
  final String peerName;
  final bool imagePreviewEnabled;
  final VoidCallback? onLongPress;
  final bool selected;

  @override
  State<MessageBubble> createState() => _MessageBubbleState();
}

class _MessageBubbleState extends State<MessageBubble>
    with SingleTickerProviderStateMixin {
  late final AnimationController _ctrl;
  late final Animation<double> _slide;
  late final Animation<double> _fade;

  @override
  void initState() {
    super.initState();
    _ctrl = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 480),
    );
    _slide = Tween(
      begin: 14.0,
      end: 0.0,
    ).animate(CurvedAnimation(parent: _ctrl, curve: Curves.easeOutCubic));
    _fade = CurvedAnimation(parent: _ctrl, curve: Curves.easeOut);
    // 软着陆：略微过冲后回稳
    _ctrl.animateWith(SpringSimulation(LunoteSprings.message, 0, 1, 0));
  }

  @override
  void dispose() {
    _ctrl.dispose();
    super.dispose();
  }

  bool get _isLink =>
      widget.message.kind == 'link' ||
      (widget.message.text.startsWith('http://') ||
          widget.message.text.startsWith('https://'));

  String get _displayText => widget.message.text;

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    final outgoing = widget.message.isOutgoing;
    return AnimatedBuilder(
      animation: _ctrl,
      builder: (context, child) {
        return Opacity(
          opacity: _fade.value,
          child: Transform.translate(
            offset: Offset(0, _slide.value),
            child: child,
          ),
        );
      },
      child: GestureDetector(
        onLongPress: widget.onLongPress,
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            Align(
              alignment: outgoing
                  ? Alignment.centerRight
                  : Alignment.centerLeft,
              child: Container(
                margin: const EdgeInsets.symmetric(vertical: 5, horizontal: 18),
                constraints: BoxConstraints(
                  // QQ-style bubbles stay readable on desktop while leaving room
                  // for the opposing side on narrow screens.
                  maxWidth: MediaQuery.of(context).size.width >= 720
                      ? 560
                      : MediaQuery.of(context).size.width * 0.86,
                ),
                padding: const EdgeInsets.symmetric(
                  horizontal: 14,
                  vertical: 10,
                ),
                decoration: BoxDecoration(
                  color: outgoing ? cc.bubbleOut : cc.bubbleIn,
                  borderRadius: BorderRadius.only(
                    topLeft: const Radius.circular(16),
                    topRight: const Radius.circular(16),
                    bottomLeft: Radius.circular(outgoing ? 16 : 4),
                    bottomRight: Radius.circular(outgoing ? 4 : 16),
                  ),
                  border: widget.selected
                      ? Border.all(color: cc.gold, width: 2)
                      : null,
                  boxShadow: const [
                    BoxShadow(
                      color: Color(0x22000000),
                      blurRadius: 6,
                      offset: Offset(0, 2),
                    ),
                  ],
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    if (!outgoing)
                      Padding(
                        padding: const EdgeInsets.only(bottom: 3),
                        child: Text(
                          widget.peerName,
                          style: TextStyle(
                            fontSize: 11,
                            color: cc.gold,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                    if (_isLink)
                      _LinkText(text: _displayText)
                    else
                      SelectableText(
                        _displayText,
                        style: TextStyle(
                          fontSize: 14.5,
                          height: 1.45,
                          color: cc.moon,
                        ),
                      ),
                    const SizedBox(height: 5),
                    Align(
                      alignment: Alignment.centerRight,
                      child: Text(
                        formatClock(widget.message.tsMs),
                        style: TextStyle(
                          fontSize: 9.5,
                          color: outgoing
                              ? cc.moon.withValues(alpha: 0.72)
                              : cc.moonDim,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
            if (widget.selected)
              Positioned(
                top: -5,
                right: outgoing ? -5 : null,
                left: outgoing ? null : -5,
                child: Icon(
                  Icons.check_circle_rounded,
                  size: 22,
                  color: cc.gold,
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _LinkText extends StatelessWidget {
  const _LinkText({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    final url = text.contains(' ') ? text.split(' ').last : text;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        if (text != url)
          Text(
            text.substring(0, text.length - url.length).trim(),
            style: TextStyle(fontSize: 14.5, height: 1.45, color: cc.moon),
          ),
        SpringButton(
          weight: SpringWeight.normal,
          onTap: () =>
              launchUrlString(url, mode: LaunchMode.externalApplication),
          child: Text(
            url,
            style: TextStyle(
              fontSize: 14.5,
              color: cc.linkBlue,
              decoration: TextDecoration.underline,
              decorationColor: cc.linkBlue,
            ),
          ),
        ),
      ],
    );
  }
}
