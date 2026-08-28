import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../core/core_client.dart';
import '../core/models.dart';

/// 应用状态：只消费核心事件与命令结果，不维护任何虚拟设备数据。
class AppState extends ChangeNotifier {
  AppState._();
  static final AppState instance = AppState._();

  final CoreClient core = CoreClient.instance;

  bool coreReady = false;
  String deviceId = '';
  String deviceName = '';

  /// 同名同 IP 新设备自动信任（核心持久化设置，默认开）
  bool autoTrust = true;

  /// 自定义接收文件保存目录（settings.json 持久化；null = 使用默认 data/downloads）
  String? defaultDownloadDir;
  String? receiveTreeUri;
  bool pinEnabled = false;

  /// 主题：dark / light / system（settings.json 持久化）
  String themeMode = 'dark';
  String conflictPolicy = 'rename';

  /// deviceId -> 设备（发现层实时更新）
  final Map<String, PeerInfo> peers = {};

  /// 通过 IP 直连且不在 UDP 发现范围内的设备。
  final Map<String, PeerInfo> _manualPeers = {};

  /// deviceId -> 信任记录
  final Map<String, TrustRecord> trusted = {};

  /// deviceId -> 消息列表（对话）
  final Map<String, List<MessageItem>> messages = {};

  /// deviceId -> 传输列表（对话内）
  final Map<String, List<TransferItem>> conversationTransfers = {};

  /// 全量传输（传输中心页）
  final List<TransferItem> allTransfers = [];

  /// 本次运行中被用户删除的空对话，避免在线/可信设备快照立即把它重新带回列表。
  final Set<String> _hiddenConversationIds = {};

  /// 本端发起的传输 → 源文件路径（供失败重试）
  final Map<String, String> sentPaths = {};

  /// 需要用户确认信任的设备（新设备连接时置位）
  String? pendingTrustDeviceId;

  /// 身份变化警告（指纹不匹配的设备）
  final Map<String, String> identityWarnings = {};

  StreamSubscription? _sub;
  Timer? _peerTimer;

  Future<void> init({
    required String dataDir,
    required String name,
    int discoveryPort = 45454,
    int tcpPort = 45455,
    String? bridgeOverride,
  }) async {
    await core.start(
      dataDir: dataDir,
      name: name,
      discoveryPort: discoveryPort,
      tcpPort: tcpPort,
      bridgeOverride: bridgeOverride,
    );
    final id = await core.call('identity');
    deviceId = id['device_id'] as String? ?? '';
    deviceName = id['name'] as String? ?? name;
    final st = await core.call('auto_trust');
    autoTrust = st['auto_trust'] == true;
    final st2 = await core.call('settings');
    final stMap = st2['settings'] as Map<String, dynamic>?;
    defaultDownloadDir = stMap?['downloads_dir'] as String?;
    receiveTreeUri = stMap?['receive_tree_uri'] as String?;
    pinEnabled = stMap?['pin_enabled'] == true;
    themeMode = stMap?['theme'] as String? ?? 'dark';
    conflictPolicy = stMap?['conflict'] as String? ?? 'rename';
    await refreshTrusted();
    await refreshConversations();
    _sub = core.events.listen(_onEvent);
    coreReady = true;
    // 核心启动后立即发现设备，可能在本端订阅事件前就发出了 peer_online
    // （广播流不缓存，错过即丢）。订阅后主动拉一次快照补上，再定时轮询兜底。
    await refreshPeers();
    _peerTimer = Timer.periodic(const Duration(seconds: 3), (_) {
      refreshPeers();
    });
    notifyListeners();
  }

  Future<void> refreshTrusted() async {
    final r = await core.call('trust_list');
    trusted.clear();
    for (final e in (r['trusted'] as List?) ?? []) {
      final rec = TrustRecord.fromJson(e as Map<String, dynamic>);
      trusted[rec.deviceId] = rec;
    }
  }

  /// 从核心拉取当前发现快照（含离线未过期设备），补上启动时错过的事件。
  /// 仅在内容变化时通知，避免无谓重建。
  Future<void> refreshPeers() async {
    final r = await core.call('peers');
    final list = (r['peers'] as List?) ?? const [];
    final next = <String, PeerInfo>{};
    for (final e in list) {
      final p = PeerInfo.fromJson(e as Map<String, dynamic>);
      next[p.deviceId] = p;
    }
    for (final entry in _manualPeers.entries) {
      next.putIfAbsent(entry.key, () => entry.value);
    }
    if (!_samePeers(next)) {
      peers
        ..clear()
        ..addAll(next);
      notifyListeners();
    }
  }

  bool _samePeers(Map<String, PeerInfo> next) {
    if (next.length != peers.length) return false;
    for (final e in next.entries) {
      final old = peers[e.key];
      if (old == null) return false;
      final n = e.value;
      if (old.deviceId != n.deviceId ||
          old.instanceId != n.instanceId ||
          old.name != n.name ||
          old.ip != n.ip ||
          old.tcpPort != n.tcpPort ||
          old.online != n.online) {
        return false;
      }
    }
    return true;
  }

  Future<void> refreshConversations() async {
    final r = await core.call('conversations');
    final nextMessages = <String, List<MessageItem>>{};
    final nextConversationTransfers = <String, List<TransferItem>>{};
    final transfersById = <String, TransferItem>{};
    for (final e in (r['conversations'] as List?) ?? []) {
      final conv = Conversation.fromJson(e as Map<String, dynamic>);
      if (conv.messages.isNotEmpty) {
        nextMessages[conv.deviceId] = conv.messages;
      }
      if (conv.transfers.isNotEmpty) {
        nextConversationTransfers[conv.deviceId] = conv.transfers;
        for (final transfer in conv.transfers) {
          transfersById[transfer.transferId] = transfer;
        }
      }
    }

    // 启动期事件可能早于 UI 订阅。持久化记录负责恢复历史，当前传输快照
    // 覆盖同 ID 的旧状态，保证传输中心和对话中的状态一致。
    final active = await core.call('transfers');
    for (final e in (active['transfers'] as List?) ?? []) {
      final transfer = TransferItem.fromJson(e as Map<String, dynamic>);
      transfersById[transfer.transferId] = transfer;
      final list = nextConversationTransfers.putIfAbsent(
        transfer.peerDeviceId,
        () => <TransferItem>[],
      );
      final index = list.indexWhere(
        (item) => item.transferId == transfer.transferId,
      );
      if (index >= 0) {
        list[index] = transfer;
      } else {
        list.add(transfer);
      }
    }

    messages
      ..clear()
      ..addAll(nextMessages);
    conversationTransfers
      ..clear()
      ..addAll(nextConversationTransfers);
    allTransfers
      ..clear()
      ..addAll(transfersById.values);
    allTransfers.sort((a, b) => a.tsMs.compareTo(b.tsMs));
    notifyListeners();
  }

  PeerInfo? peer(String deviceId) => peers[deviceId];

  String peerName(String deviceId) {
    final p = peers[deviceId];
    if (p != null && p.name.isNotEmpty) return p.name;
    final t = trusted[deviceId];
    if (t != null && t.name.isNotEmpty) return t.name;
    return '未知设备';
  }

  bool isTrusted(String deviceId) => trusted[deviceId]?.trusted ?? false;

  List<MessageItem> messagesOf(String deviceId) =>
      messages[deviceId] ?? const [];

  List<TransferItem> transfersOf(String deviceId) =>
      conversationTransfers[deviceId] ?? const [];

  /// 对话排序（按最后活动时间）
  List<String> get conversationOrder {
    final ids = <String>{
      ...messages.entries.where((e) => e.value.isNotEmpty).map((e) => e.key),
      ...conversationTransfers.entries
          .where((e) => e.value.isNotEmpty)
          .map((e) => e.key),
      ...trusted.keys,
      ...peers.keys,
    };
    ids.removeAll(_hiddenConversationIds);
    final list = ids.toList();
    list.sort((a, b) {
      final la = lastActivity(a);
      final lb = lastActivity(b);
      return lb.compareTo(la);
    });
    return list;
  }

  int lastActivity(String deviceId) {
    var t = 0;
    for (final m in messagesOf(deviceId)) {
      if (m.tsMs > t) t = m.tsMs;
    }
    for (final tr in transfersOf(deviceId)) {
      if (tr.tsMs > t) t = tr.tsMs;
    }
    return t;
  }

  void _onEvent(Map<String, dynamic> e) {
    switch (e['event']) {
      case 'peer_online':
        _upsertPeer(PeerInfo.fromJson(e));
      case 'peer_offline':
        final id = e['device_id'] as String?;
        if (id != null && peers[id] != null) {
          final old = peers[id]!;
          peers[id] = PeerInfo(
            deviceId: old.deviceId,
            instanceId: old.instanceId,
            name: old.name,
            ip: old.ip,
            tcpPort: old.tcpPort,
            online: false,
          );
        }
      case 'peer_name_changed':
        final id = e['device_id'] as String?;
        final newName = e['new_name'] as String?;
        if (id != null && newName != null && peers[id] != null) {
          final old = peers[id]!;
          peers[id] = PeerInfo(
            deviceId: old.deviceId,
            instanceId: old.instanceId,
            name: newName,
            ip: old.ip,
            tcpPort: old.tcpPort,
            online: old.online,
          );
        }
      case 'peer_connected':
        final id = e['device_id'] as String?;
        if (id != null) {
          final name = e['name'] as String? ?? '';
          final ip = peers[id]?.ip ?? '';
          final port = peers[id]?.tcpPort ?? 0;
          peers[id] = PeerInfo(
            deviceId: id,
            instanceId: peers[id]?.instanceId ?? '',
            name: name,
            ip: ip,
            tcpPort: port,
            online: true,
          );
          if (e['is_new_device'] == true) {
            pendingTrustDeviceId = id;
          }
          unawaited(refreshTrusted());
        }
      case 'peer_disconnected':
        final id = e['device_id'] as String?;
        final old = id == null ? null : peers[id];
        if (id != null && old != null) {
          final offline = PeerInfo(
            deviceId: old.deviceId,
            instanceId: old.instanceId,
            name: old.name,
            ip: old.ip,
            tcpPort: old.tcpPort,
            online: false,
          );
          peers[id] = offline;
          if (_manualPeers.containsKey(id)) _manualPeers[id] = offline;
        }
      case 'identity_changed':
        final id = e['device_id'] as String?;
        if (id != null) {
          identityWarnings[id] = (e['new_fingerprint'] as String? ?? '')
              .substring(0, 12);
          unawaited(refreshTrusted());
        }
      case 'message_received':
      case 'message_sent':
        final id = e['device_id'] as String?;
        if (id == null) break;
        final list = messages[id] ?? [];
        list.add(
          MessageItem(
            id: e['message_id'] as String? ?? '',
            direction: e['event'] == 'message_received'
                ? 'incoming'
                : 'outgoing',
            kind: e['kind'] as String? ?? 'text',
            text: e['text'] as String? ?? '',
            url: e['url'] as String?,
            tsMs: (e['ts_ms'] as num?)?.toInt() ?? 0,
          ),
        );
        messages[id] = list;
        _hiddenConversationIds.remove(id);
      case 'transfer_update':
        final t = TransferItem.fromJson(e);
        _hiddenConversationIds.remove(t.peerDeviceId);
        _upsertTransfer(t);
        if (t.isDone && receiveTreeUri != null && t.localPath != null) {
          unawaited(_exportReceivedToTree(t));
        }
        if (t.state == 'done' || t.state == 'failed' || t.isOffered) {
          unawaited(_notifyTransfer(t));
        }
      case 'trust_changed':
        unawaited(refreshTrusted());
      case 'records_changed':
        unawaited(refreshConversations());
    }
    notifyListeners();
  }

  Future<void> _notifyTransfer(TransferItem t) async {
    if (!Platform.isAndroid) return;
    try {
      final title = t.isOffered
          ? '收到文件：${t.fileName}'
          : (t.isDone ? '传输完成：${t.fileName}' : '传输失败：${t.fileName}');
      final body = t.isOffered
          ? '打开月笺以选择接收目录'
          : (t.isDone ? '文件校验通过' : (t.error ?? '可重试传输'));
      await const MethodChannel('com.lunote.lunote_app/platform').invokeMethod(
        'notifyTransfer',
        {'title': title, 'body': body},
      );
    } catch (_) {
      // 通知不可用不影响传输本身。
    }
  }

  void _upsertPeer(PeerInfo p) {
    peers[p.deviceId] = p;
  }

  void _upsertTransfer(TransferItem t) {
    final list = conversationTransfers[t.peerDeviceId] ?? [];
    final idx = list.indexWhere((x) => x.transferId == t.transferId);
    if (idx >= 0) {
      list[idx] = t;
    } else {
      list.add(t);
    }
    conversationTransfers[t.peerDeviceId] = list;
    final allIdx = allTransfers.indexWhere((x) => x.transferId == t.transferId);
    if (allIdx >= 0) {
      allTransfers[allIdx] = t;
    } else {
      allTransfers.add(t);
    }
  }

  // ---------- 动作 ----------

  Future<String?> sendText(String deviceId, String text) async {
    _hiddenConversationIds.remove(deviceId);
    final r = await core.call('send_text', {
      'device_id': deviceId,
      'text': text,
    });
    return r['ok'] == true ? null : (r['error'] as String? ?? '发送失败');
  }

  Future<String?> sendLink(String deviceId, String url) async {
    _hiddenConversationIds.remove(deviceId);
    final r = await core.call('send_link', {'device_id': deviceId, 'url': url});
    return r['ok'] == true ? null : (r['error'] as String? ?? '发送失败');
  }

  Future<String?> sendFile(String deviceId, String path) async {
    _hiddenConversationIds.remove(deviceId);
    final r = await core.call('send_file', {
      'device_id': deviceId,
      'path': path,
    });
    if (r['ok'] == true) {
      for (final id in (r['transfer_ids'] as List?) ?? []) {
        sentPaths[id as String] = path;
      }
      return null;
    }
    return r['error'] as String? ?? '发送失败';
  }

  /// 通过虚拟局域网或其它可路由网络的地址直连，并返回握手确认的设备 ID。
  Future<({String? deviceId, String? error})> connectAddress(
    String host,
    int port,
  ) async {
    final r = await core.call('connect_address', {'host': host, 'port': port});
    if (r['ok'] != true) {
      return (deviceId: null, error: r['error'] as String? ?? '连接失败');
    }
    await refreshTrusted();
    final id = r['device_id'] as String?;
    final name = r['name'] as String? ?? '直连设备';
    if (id != null && id.isNotEmpty) {
      final peer = PeerInfo(
        deviceId: id,
        instanceId: '',
        name: name,
        ip: host,
        tcpPort: port,
        online: true,
      );
      _manualPeers[id] = peer;
      peers[id] = peer;
      notifyListeners();
    }
    return (
      deviceId: id,
      error: id == null || id.isEmpty ? '连接成功，但未获得设备身份' : null,
    );
  }

  Future<String?> trustDevice(
    String deviceId, {
    bool trusted = true,
    String? name,
  }) async {
    pendingTrustDeviceId = null;
    final r = await core.call('trust', {
      'device_id': deviceId,
      'trusted': trusted,
      'name': name ?? peerName(deviceId),
    });
    await refreshTrusted();
    return r['ok'] == true ? null : (r['error'] as String? ?? '操作失败');
  }

  /// 移除本地设备身份/信任记录；不影响对端，也不删除对话记录。
  Future<String?> removeDevice(String deviceId) async {
    final r = await core.call('remove_device', {'device_id': deviceId});
    if (r['ok'] == true) {
      trusted.remove(deviceId);
      notifyListeners();
      return null;
    }
    return r['error'] as String? ?? '移除失败';
  }

  Future<String?> acceptTransfer(String transferId, String dest) async {
    final r = await core.call('accept', {
      'transfer_id': transferId,
      'dest': dest,
    });
    return r['ok'] == true ? null : (r['error'] as String? ?? '接受失败');
  }

  Future<String?> rejectTransfer(String transferId, String reason) async {
    final r = await core.call('reject', {
      'transfer_id': transferId,
      'reason': reason,
    });
    return r['ok'] == true ? null : (r['error'] as String? ?? '拒绝失败');
  }

  /// 删除与某设备的整个对话（本地记录）
  Future<String?> deleteConversation(String deviceId, {String? name}) async {
    return deleteConversations([deviceId]);
  }

  /// 在核心数据库事务中批量删除对话，避免出现部分删除。
  Future<String?> deleteConversations(Iterable<String> deviceIds) async {
    final ids = deviceIds.toSet().toList();
    if (ids.isEmpty) return null;
    final r = await core.call('delete_conversations', {'device_ids': ids});
    if (r['ok'] != true) {
      return r['error'] as String? ?? '删除失败';
    }
    for (final id in ids) {
      messages.remove(id);
      conversationTransfers.remove(id);
      _hiddenConversationIds.add(id);
    }
    allTransfers.removeWhere((t) => ids.contains(t.peerDeviceId));
    notifyListeners();
    return null;
  }

  /// 返回实际接收目录；未自定义时解析核心数据目录下的 downloads。
  Future<String?> resolvedDownloadDir() async {
    final configured = defaultDownloadDir;
    if (configured != null && configured.isNotEmpty) return configured;
    final r = await core.call('data_dir');
    final dataDir = r['data_dir'] as String?;
    if (dataDir == null || dataDir.isEmpty) return null;
    return '$dataDir${Platform.pathSeparator}downloads';
  }

  Future<String?> cancelTransfer(String transferId) async {
    final r = await core.call('cancel', {'transfer_id': transferId});
    return r['ok'] == true ? null : (r['error'] as String? ?? '取消失败');
  }

  Future<String?> pauseTransfer(String transferId) async {
    final r = await core.call('pause', {'transfer_id': transferId});
    return r['ok'] == true ? null : (r['error'] as String? ?? '暂停失败');
  }

  Future<String?> resumeTransfer(String transferId) async {
    final r = await core.call('resume', {'transfer_id': transferId});
    return r['ok'] == true ? null : (r['error'] as String? ?? '继续失败');
  }

  Future<String?> renameDevice(String name) async {
    final r = await core.call('rename', {'name': name});
    if (r['ok'] == true) {
      deviceName = name;
    }
    return r['ok'] == true ? null : (r['error'] as String? ?? '改名失败');
  }

  /// 切换“同名同 IP 新设备自动信任”（核心持久化）
  Future<String?> setAutoTrust(bool enabled) async {
    final r = await core.call('set_auto_trust', {'enabled': enabled});
    if (r['ok'] == true) {
      autoTrust = enabled;
      notifyListeners();
      return null;
    }
    return r['error'] as String? ?? '设置失败';
  }

  /// 设置自定义接收目录（核心持久化）；传 null 恢复默认
  Future<String?> setDownloadDir(String? dir) async {
    final r = await core.call('set_downloads_dir', {'dir': dir});
    if (r['ok'] == true) {
      defaultDownloadDir = dir;
      notifyListeners();
      return null;
    }
    return r['error'] as String? ?? '设置失败';
  }

  Future<String?> pickReceiveFolder() async {
    if (!Platform.isAndroid) return '仅 Android 支持 SAF 接收目录';
    try {
      final uri = await const MethodChannel('com.lunote.lunote_app/platform')
          .invokeMethod<String>('pickReceiveFolder');
      if (uri == null || uri.isEmpty) return '未选择目录';
      receiveTreeUri = uri;
      await core.call('set_receive_tree_uri', {'uri': uri});
      notifyListeners();
      return null;
    } catch (e) {
      return '选择接收目录失败：$e';
    }
  }

  Future<String?> setPin(String? pin) async {
    final r = await core.call('set_pin', {'pin': pin});
    if (r['ok'] == true) {
      pinEnabled = pin != null && pin.isNotEmpty;
      notifyListeners();
      return null;
    }
    return r['error'] as String? ?? '设置应用锁失败';
  }

  Future<bool> verifyPin(String pin) async {
    final r = await core.call('verify_pin', {'pin': pin});
    return r['valid'] == true;
  }

  Future<void> _exportReceivedToTree(TransferItem t) async {
    try {
      await const MethodChannel('com.lunote.lunote_app/platform').invokeMethod(
        'exportToTree', {'path': t.localPath, 'treeUri': receiveTreeUri},
      );
    } catch (_) {}
  }

  /// 设置主题：dark / light / system（核心持久化）
  Future<String?> setTheme(String mode) async {
    final r = await core.call('set_theme', {'theme': mode});
    if (r['ok'] == true) {
      themeMode = mode;
      notifyListeners();
      return null;
    }
    return r['error'] as String? ?? '设置失败';
  }

  Future<String?> setConflictPolicy(String policy) async {
    final r = await core.call('set_conflict_policy', {'policy': policy});
    if (r['ok'] == true) {
      conflictPolicy = policy;
      notifyListeners();
      return null;
    }
    return r['error'] as String? ?? '设置失败';
  }

  Future<Map<String, dynamic>> diagnostics() async {
    final r = await core.call('diagnostics');
    return (r['diagnostics'] as Map?)?.cast<String, dynamic>() ?? <String, dynamic>{};
  }

  Future<Map<String, dynamic>> exportRecords(String password, String outPath) =>
      core.call('export', {'password': password, 'out': outPath});

  Future<Map<String, dynamic>> importRecords(String password, String inPath) =>
      core.call('import', {'password': password, 'input': inPath});

  Future<void> disposeCore() async {
    _peerTimer?.cancel();
    _peerTimer = null;
    await _sub?.cancel();
    _sub = null;
    await core.stop();
    coreReady = false;
  }
}
