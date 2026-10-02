import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:lunote_app/main.dart';
import 'package:lunote_app/src/state/app_state.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('Windows与Android实际互通：密码笔记、临时消息和文件完整性', (tester) async {
    const host = String.fromEnvironment('LUNOTE_INTEROP_HOST');
    expect(
      host,
      isNotEmpty,
      reason:
          'Start tests/android_interop_host.py and provide LUNOTE_INTEROP_HOST',
    );
    final dir = await Directory.systemTemp.createTemp(
      'lunote_interop_android_',
    );
    final state = AppState.instance;
    await tester.pumpWidget(
      LunoteApp(
        dataDirOverride: dir.path,
        nameOverride: 'interop-android-v2',
        tcpPortOverride: 45890,
      ),
    );
    for (var i = 0; i < 100 && !state.coreReady; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    expect(state.coreReady, true);
    try {
      final connected = await state.core.call('connect_address', {
        'host': host,
        'port': 45889,
      });
      expect(connected['ok'], true, reason: connected['error']?.toString());
      final peer = connected['device_id'] as String;
      expect((await state.core.call('trust', {'device_id': peer}))['ok'], true);
      expect(
        (await state.core.call('set_note_peers', {
          'device_ids': [peer],
        }))['ok'],
        true,
      );
      Map<String, dynamic>? note;
      for (var i = 0; i < 180 && note == null; i++) {
        final result = await state.core.call('notes');
        for (final item in result['notes'] as List) {
          if (item['title'] == 'Cross-platform locked note')
            note = (item as Map).cast<String, dynamic>();
        }
        if (note == null) await tester.pump(const Duration(milliseconds: 200));
      }
      expect(note, isNotNull);
      expect(note!['body'], isNull);
      final unlocked = await state.core.call('unlock_note', {
        'note_id': note['id'],
        'password': 'interop-password-123',
      });
      expect(unlocked['body'], 'windows-secret');
      expect(
        (await state.core.call('save_note', {
          'note': {...note, 'body': 'android-edited-secret'},
          'password': 'interop-password-123',
        }))['ok'],
        true,
      );
      String? threadId;
      Map<String, dynamic>? received;
      var replied = false;
      for (var i = 0; i < 300 && received == null; i++) {
        final threads = (await state.core.call('threads'))['threads'] as List;
        if (threads.isNotEmpty) threadId = threads.single['id'] as String;
        if (threadId != null && !replied) {
          expect(
            (await state.core.call('send_thread_text', {
              'device_id': peer,
              'thread_id': threadId,
              'text': 'android-thread-reply',
            }))['ok'],
            true,
          );
          replied = true;
        }
        final transfers =
            (await state.core.call('transfers'))['transfers'] as List;
        for (final item in transfers) {
          if (item['state'] == 'offered') {
            expect(
              (await state.core.call('accept', {
                'transfer_id': item['transfer_id'],
                'dest': '${dir.path}/received',
              }))['ok'],
              true,
            );
          } else if (item['state'] == 'done') {
            received = (item as Map).cast<String, dynamic>();
          } else if (item['state'] == 'failed') {
            fail('Transfer failed: ${item['error']}');
          }
        }
        if (received == null)
          await tester.pump(const Duration(milliseconds: 200));
      }
      expect(received, isNotNull);
      expect(received!['thread_id'], threadId);
      final bytes = await File(received['local_path'] as String).readAsBytes();
      expect(bytes.length, 4 * 1024 * 1024);
      for (var i = 0; i < bytes.length; i++) {
        if (bytes[i] != i % 251) fail('File integrity mismatch at $i');
      }
      final conversations =
          (await state.core.call('conversations'))['conversations'] as List;
      final main = conversations.singleWhere((c) => c['device_id'] == peer);
      expect(
        (main['messages'] as List).any(
          (m) => m['text'] == 'windows-main-message',
        ),
        true,
      );
      expect(main['transfers'], isEmpty);
      expect(
        (main['messages'] as List).any(
          (m) => m['text'] == 'windows-thread-message',
        ),
        false,
      );
      final temporary = conversations.singleWhere(
        (c) => c['device_id'] != peer,
      );
      expect(
        (temporary['messages'] as List).any(
          (m) => m['text'] == 'windows-thread-message',
        ),
        true,
      );
      expect(tester.takeException(), isNull);
    } finally {
      await tester.pumpWidget(const SizedBox());
      await tester.pump(const Duration(seconds: 3));
      await dir.delete(recursive: true);
    }
  });
}
