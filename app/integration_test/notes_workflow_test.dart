import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:lunote_app/main.dart';
import 'package:lunote_app/src/state/app_state.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('真实核心笔记密码与重启恢复，双端命令字段一致', (tester) async {
    final dir = await Directory.systemTemp.createTemp('lunote_notes_workflow_');
    final state = AppState.instance;
    final capture = GlobalKey();
    await tester.pumpWidget(
      RepaintBoundary(
        key: capture,
        child: LunoteApp(
          dataDirOverride: dir.path,
          nameOverride: 'notes integration',
          tcpPortOverride: 45886,
        ),
      ),
    );
    for (var i = 0; i < 100 && !state.coreReady; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    expect(state.coreReady, true);
    final saved = await state.core.call('save_note', {
      'note': {
        'id': '',
        'title': '集成测试笔记',
        'body': 'secret-api-key-123',
        'locked': null,
        'clock': <String, dynamic>{},
        'deleted': false,
        'pinned': true,
        'group': '测试',
        'order': 0,
        'shielded': true,
      },
      'new_password': 'integration-pass-123',
    });
    expect(saved['ok'], true, reason: saved['error']?.toString());
    final note = (saved['note'] as Map).cast<String, dynamic>();
    expect(note['body'], isNull);
    expect(note['locked'], isNotNull);
    final bad = await state.core.call('unlock_note', {
      'note_id': note['id'],
      'password': 'wrong-pass',
    });
    expect(bad['ok'], false);
    final unlocked = await state.core.call('unlock_note', {
      'note_id': note['id'],
      'password': 'integration-pass-123',
    });
    expect(unlocked['body'], 'secret-api-key-123');
    await state.refreshNotes();
    expect(state.notes.single.title, '集成测试笔记');
    await tester.pumpAndSettle();
    await tester.tap(find.text('笔记').first);
    await tester.pumpAndSettle();
    expect(find.text('集成测试笔记'), findsOneWidget);
    expect(find.text('secret-api-key-123'), findsNothing);
    expect(tester.takeException(), isNull);
    final screenshotDir = Platform.environment['LUNOTE_SCREENSHOT_DIR'];
    if (screenshotDir != null && screenshotDir.isNotEmpty) {
      final boundary =
          capture.currentContext!.findRenderObject()! as RenderRepaintBoundary;
      final image = await boundary.toImage(pixelRatio: 1.5);
      final png = await image.toByteData(format: ui.ImageByteFormat.png);
      await Directory(screenshotDir).create(recursive: true);
      await File('$screenshotDir/windows-notes-2.0.png')
          .writeAsBytes(png!.buffer.asUint8List());
      image.dispose();
    }
    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(seconds: 3));
    await tester.pumpWidget(
      LunoteApp(
        dataDirOverride: dir.path,
        nameOverride: 'notes integration',
        tcpPortOverride: 45886,
      ),
    );
    for (var i = 0; i < 100 && !state.coreReady; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    expect(state.coreReady, true);
    await state.refreshNotes();
    expect(state.notes.single.id, note['id']);
    expect(state.notes.single.locked, true);
    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(seconds: 3));
    await dir.delete(recursive: true);
  });
}
