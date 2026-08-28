import 'package:flutter/material.dart';

/// 月笺 Lunote 设计体系：月光、信笺、安静连接。
/// 深/浅两套色板，通过 ThemeExtension 挂到 ThemeData，页面用 LunoteColors.of(context) 取当前色。

@immutable
class LunoteColors extends ThemeExtension<LunoteColors> {
  const LunoteColors({
    required this.night,
    required this.nightRaised,
    required this.nightSoft,
    required this.moon,
    required this.moonDim,
    required this.gold,
    required this.goldDeep,
    required this.online,
    required this.offline,
    required this.warn,
    required this.bubbleOut,
    required this.bubbleIn,
    required this.linkBlue,
    this.isGlass = false,
  });

  final Color night;
  final Color nightRaised;
  final Color nightSoft;
  final Color moon;
  final Color moonDim;
  final Color gold;
  final Color goldDeep;
  final Color online;
  final Color offline;
  final Color warn;
  final Color bubbleOut;
  final Color bubbleIn;
  final Color linkBlue;
  final bool isGlass;

  /// 深色：深蓝夜色 + 月白 + 月牙金
  static const LunoteColors dark = LunoteColors(
    night: Color(0xFF1A1F2E),
    nightRaised: Color(0xFF222839),
    nightSoft: Color(0xFF2A3145),
    moon: Color(0xFFF4F1E8),
    moonDim: Color(0xFFC9C5B8),
    gold: Color(0xFFE8C87A),
    goldDeep: Color(0xFFD4AC55),
    online: Color(0xFF7BD389),
    offline: Color(0xFF6B7280),
    warn: Color(0xFFE58A7A),
    bubbleOut: Color(0xFF3A4460),
    bubbleIn: Color(0xFF2B3145),
    linkBlue: Color(0xFF9DB8E8),
  );

  /// 浅色：暖白纸感 + 墨色文字 + 琥珀点缀（对比度更高，白天/户外更清晰）
  static const LunoteColors light = LunoteColors(
    night: Color(0xFFF3F1EA),
    nightRaised: Color(0xFFFFFFFF),
    nightSoft: Color(0xFFE5E2D8),
    moon: Color(0xFF242833),
    moonDim: Color(0xFF5F6572),
    gold: Color(0xFF9A7219),
    goldDeep: Color(0xFF7F5D12),
    online: Color(0xFF1F8A4C),
    offline: Color(0xFF9AA0AA),
    warn: Color(0xFFC0392B),
    bubbleOut: Color(0xFFE9E6DA),
    bubbleIn: Color(0xFFFFFFFF),
    linkBlue: Color(0xFF2F55B0),
  );

  /// 月光玻璃：冷白磨砂基底，莓果与青绿色作灵动点缀。
  static const LunoteColors glass = LunoteColors(
    night: Color(0x55EAF1F5),
    nightRaised: Color(0xA8FFFFFF),
    nightSoft: Color(0x88DDE7EC),
    moon: Color(0xFF172238),
    moonDim: Color(0xFF5B687D),
    gold: Color(0xFFD75089),
    goldDeep: Color(0xFFB83A73),
    online: Color(0xFF278866),
    offline: Color(0xFF7F8997),
    warn: Color(0xFFD94E61),
    bubbleOut: Color(0xD93DB8C7),
    bubbleIn: Color(0xC9FFFFFF),
    linkBlue: Color(0xFF276AA8),
    isGlass: true,
  );
  static LunoteColors of(BuildContext context) =>
      Theme.of(context).extension<LunoteColors>() ?? LunoteColors.dark;

  @override
  LunoteColors copyWith({
    Color? night,
    Color? nightRaised,
    Color? nightSoft,
    Color? moon,
    Color? moonDim,
    Color? gold,
    Color? goldDeep,
    Color? online,
    Color? offline,
    Color? warn,
    Color? bubbleOut,
    Color? bubbleIn,
    Color? linkBlue,
    bool? isGlass,
  }) {
    return LunoteColors(
      night: night ?? this.night,
      nightRaised: nightRaised ?? this.nightRaised,
      nightSoft: nightSoft ?? this.nightSoft,
      moon: moon ?? this.moon,
      moonDim: moonDim ?? this.moonDim,
      gold: gold ?? this.gold,
      goldDeep: goldDeep ?? this.goldDeep,
      online: online ?? this.online,
      offline: offline ?? this.offline,
      warn: warn ?? this.warn,
      bubbleOut: bubbleOut ?? this.bubbleOut,
      bubbleIn: bubbleIn ?? this.bubbleIn,
      linkBlue: linkBlue ?? this.linkBlue,
      isGlass: isGlass ?? this.isGlass,
    );
  }

  @override
  LunoteColors lerp(ThemeExtension<LunoteColors>? other, double t) {
    if (other is! LunoteColors) return this;
    return LunoteColors(
      night: Color.lerp(night, other.night, t)!,
      nightRaised: Color.lerp(nightRaised, other.nightRaised, t)!,
      nightSoft: Color.lerp(nightSoft, other.nightSoft, t)!,
      moon: Color.lerp(moon, other.moon, t)!,
      moonDim: Color.lerp(moonDim, other.moonDim, t)!,
      gold: Color.lerp(gold, other.gold, t)!,
      goldDeep: Color.lerp(goldDeep, other.goldDeep, t)!,
      online: Color.lerp(online, other.online, t)!,
      offline: Color.lerp(offline, other.offline, t)!,
      warn: Color.lerp(warn, other.warn, t)!,
      bubbleOut: Color.lerp(bubbleOut, other.bubbleOut, t)!,
      bubbleIn: Color.lerp(bubbleIn, other.bubbleIn, t)!,
      linkBlue: Color.lerp(linkBlue, other.linkBlue, t)!,
      isGlass: t < 0.5 ? isGlass : other.isGlass,
    );
  }
}

/// 向后兼容：保留旧静态色板常量（指向深色），新代码请用 LunoteColors.of(context)。
class Luna {
  Luna._();
}

class LunoteSprings {
  LunoteSprings._();

  /// 主要按钮：饱满有力量（按压缩小，松手回弹带过冲）
  static const SpringDescription primary = SpringDescription(
    mass: 0.8,
    stiffness: 620,
    damping: 21,
  );

  /// 普通按钮：轻巧
  static const SpringDescription normal = SpringDescription(
    mass: 0.7,
    stiffness: 780,
    damping: 24,
  );

  /// 图标按钮：短促灵敏
  static const SpringDescription icon = SpringDescription(
    mass: 0.6,
    stiffness: 1000,
    damping: 26,
  );

  /// 面板/弹窗：展开后轻微回稳
  static const SpringDescription panel = SpringDescription(
    mass: 1.1,
    stiffness: 300,
    damping: 24,
  );

  /// 消息落入：软着陆
  static const SpringDescription message = SpringDescription(
    mass: 0.9,
    stiffness: 420,
    damping: 19,
  );
}

class LunoteTheme {
  LunoteTheme._();

  static ThemeData dark() => _build(Brightness.dark, LunoteColors.dark);

  static ThemeData light() => _build(Brightness.light, LunoteColors.light);

  static ThemeData glass() => _build(Brightness.light, LunoteColors.glass);

  static ThemeData _build(Brightness brightness, LunoteColors c) {
    final base = ThemeData(brightness: brightness, useMaterial3: true);
    final scheme = ColorScheme.fromSeed(
      seedColor: c.gold,
      brightness: brightness,
      surface: c.nightRaised,
    );
    return base.copyWith(
      scaffoldBackgroundColor: c.isGlass ? Colors.transparent : c.night,
      colorScheme: scheme,
      extensions: [c],
      textTheme: base.textTheme.apply(
        bodyColor: c.moon,
        displayColor: c.moon,
        fontFamily: null,
      ),
      dividerColor: c.nightSoft,
      splashFactory: InkRipple.splashFactory,
      dialogTheme: DialogThemeData(
        backgroundColor: c.nightRaised,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(20)),
      ),
      snackBarTheme: SnackBarThemeData(
        backgroundColor: c.nightSoft,
        contentTextStyle: TextStyle(color: c.moon),
        behavior: SnackBarBehavior.floating,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
      ),
      tooltipTheme: TooltipThemeData(
        decoration: BoxDecoration(
          color: c.nightSoft,
          borderRadius: const BorderRadius.all(Radius.circular(8)),
        ),
        textStyle: TextStyle(color: c.moon, fontSize: 12),
      ),
    );
  }
}
