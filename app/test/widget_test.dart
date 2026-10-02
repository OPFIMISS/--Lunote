// 月笺 Lunote 基础组件测试（不依赖核心网络层）。
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';

import 'package:lunote_app/src/ui/lunote_theme.dart';
import 'package:lunote_app/src/ui/widgets/spring_button.dart';
import 'package:lunote_app/src/ui/widgets/transfer_tile.dart';
import 'package:lunote_app/src/core/models.dart';
import 'package:lunote_app/src/state/app_state.dart';
import 'package:lunote_app/src/ui/pages/notes_page.dart';
import 'package:lunote_app/src/ui/widgets/message_bubble.dart';

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
  testWidgets('防窥揭示后进入后台重新模糊', (tester) async {
    final state=AppState.instance;
    state.notes
      ..clear()
      ..add(NoteItem.fromJson({'id':'shielded','title':'private note','body':'private body',
        'locked':null,'clock':{'a':1},'deleted':false,'pinned':false,
        'group':'','order':0,'shielded':true}));
    addTearDown(state.notes.clear);
    await tester.pumpWidget(ChangeNotifierProvider<AppState>.value(value:state,
      child:MaterialApp(theme:LunoteTheme.light(),home:const Scaffold(body:NotesPage()))));
    await tester.pumpAndSettle();
    final filter=find.byType(ImageFiltered);
    expect(tester.widget<ImageFiltered>(filter).imageFilter.toString(),contains('7.0'));
    await tester.tap(find.text('private body'));
    await tester.pump(const Duration(milliseconds:60));
    await tester.tap(find.text('private body'));
    await tester.pumpAndSettle();
    expect(tester.widget<ImageFiltered>(filter).imageFilter.toString(),contains('0.0'));
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump();
    expect(tester.widget<ImageFiltered>(filter).imageFilter.toString(),contains('7.0'));
  });
  testWidgets('离线气泡显示待送达而不是已读', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: LunoteTheme.light(),
        home: Scaffold(
          body: MessageBubble(
            message: MessageItem(
              id: 'queued',
              direction: 'outgoing',
              kind: 'text',
              text: 'offline text',
              tsMs: 1,
              delivery: 'pending',
            ),
            peerName: 'peer',
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.textContaining('待送达'), findsOneWidget);
    expect(find.textContaining('已读'), findsNothing);
  });

  testWidgets('笔记在手机和PC尺寸可展开收起且无排版溢出', (tester) async {
    final state = AppState.instance;
    state.notes.clear();
    for (var index = 0; index < 5; index++) {
      state.notes.add(
        NoteItem.fromJson({
          'id': 'note-$index',
          'title': '中文标题及长名称测试 $index',
          'body': index == 1
              ? null
              : '多行内容\n中文内容\n第三行\n第四行\n第五行\n第六行\n第七行\n第八行',
          'locked': index == 1 ? <String, dynamic>{} : null,
          'clock': {'a': 1},
          'deleted': false,
          'pinned': index == 0,
          'group': '分组',
          'order': index,
          'shielded': index == 2,
        }),
      );
    }
    addTearDown(() {
      state.notes.clear();
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });
    tester.view.devicePixelRatio = 1;
    for (final size in [
      const Size(360, 800),
      const Size(720, 900),
      const Size(1280, 800),
    ]) {
      tester.view.physicalSize = size;
      await tester.pumpWidget(
        ChangeNotifierProvider<AppState>.value(
          value: state,
          child: MaterialApp(
            theme: LunoteTheme.light(),
            home: const Scaffold(body: NotesPage()),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      expect(find.byIcon(Icons.lock_outline_rounded), findsOneWidget);
      await tester.tap(find.byTooltip('仅显示标题'));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      expect(find.textContaining('第三行'), findsNothing);
      await tester.tap(find.byTooltip('方形卡片'));
      await tester.pumpAndSettle();
    }
  });
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
