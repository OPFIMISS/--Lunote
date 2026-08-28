// 月笺 Lunote 基础组件测试（不依赖核心网络层）。
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:lunote_app/src/ui/lunote_theme.dart';
import 'package:lunote_app/src/ui/widgets/spring_button.dart';
import 'package:lunote_app/src/ui/widgets/transfer_tile.dart';
import 'package:lunote_app/src/core/models.dart';

TransferItem offeredTransfer(String direction) => TransferItem(
  transferId: 'transfer-$direction',
  peerDeviceId: 'peer',
  direction: direction,
  state: 'offered',
  fileName: 'photo.png',
  fileSize: 1024,
  transferred: 0,
  speedBps: 0,
  resumeOffset: 0,
  tsMs: 0,
);

Widget transferHarness(TransferItem transfer) => MaterialApp(
  theme: LunoteTheme.dark(),
  home: Scaffold(
    body: TransferTile(
      transfer: transfer,
      onAccept: () {},
      onReject: () {},
      onCancel: () {},
      onRetry: () {},
    ),
  ),
);

TransferItem activeTransfer() => TransferItem(
  transferId: 'active-transfer',
  peerDeviceId: 'peer',
  direction: 'outgoing',
  state: 'in_progress',
  fileName: 'archive.zip',
  fileSize: 100 * 1024 * 1024,
  transferred: 50 * 1024 * 1024,
  speedBps: 10 * 1024 * 1024,
  resumeOffset: 0,
  tsMs: 0,
);

void main() {
  testWidgets('SpringButton 可点击且带弹簧动画', (tester) async {
    var tapped = 0;
    await tester.pumpWidget(
      MaterialApp(
        theme: LunoteTheme.dark(),
        home: Scaffold(
          body: Center(
            child: SpringButton(
              weight: SpringWeight.primary,
              onTap: () => tapped++,
              child: const Text('点击'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('点击'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(tapped, 1);
  });

  testWidgets('主题为深色月笺配色', (tester) async {
    final theme = LunoteTheme.dark();
    expect(theme.scaffoldBackgroundColor, const Color(0xFF1A1F2E));
  });

  testWidgets('发送方等待确认时只显示取消发送', (tester) async {
    await tester.pumpWidget(transferHarness(offeredTransfer('outgoing')));
    expect(find.text('等待对方确认'), findsOneWidget);
    expect(find.text('取消发送'), findsOneWidget);
    expect(find.text('接收'), findsNothing);
    expect(find.text('拒绝'), findsNothing);
  });

  testWidgets('接收方收到文件时显示接收与拒绝', (tester) async {
    await tester.pumpWidget(transferHarness(offeredTransfer('incoming')));
    expect(find.text('等待你确认'), findsOneWidget);
    expect(find.text('接收'), findsOneWidget);
    expect(find.text('拒绝'), findsOneWidget);
    expect(find.text('取消发送'), findsNothing);
  });

  testWidgets('传输中显示速度、预计剩余时间与平滑百分比', (tester) async {
    await tester.pumpWidget(transferHarness(activeTransfer()));
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.textContaining('传输速度 10.0 MB/s'), findsOneWidget);
    expect(find.textContaining('预计剩余 5s'), findsOneWidget);
    expect(find.text('50%'), findsOneWidget);
  });
}
