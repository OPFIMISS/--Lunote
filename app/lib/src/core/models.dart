/// 核心事件与数据的类型化模型（与 Rust serde snake_case 字段对应）。
library;

class PeerInfo {
  final String deviceId;
  final String instanceId;
  final String name;
  final String ip;
  final int tcpPort;
  final bool online;

  PeerInfo({
    required this.deviceId,
    required this.instanceId,
    required this.name,
    required this.ip,
    required this.tcpPort,
    required this.online,
  });

  factory PeerInfo.fromJson(Map<String, dynamic> j) => PeerInfo(
    deviceId: j['device_id'] as String? ?? '',
    instanceId: j['instance_id'] as String? ?? '',
    name: j['name'] as String? ?? '未命名设备',
    ip: j['ip'] as String? ?? '',
    tcpPort: (j['tcp_port'] as num?)?.toInt() ?? 0,
    online: j['online'] as bool? ?? false,
  );
}

class TrustRecord {
  final String deviceId;
  final String name;
  final String fingerprint;
  final bool trusted;
  final int firstSeenMs;
  final int lastSeenMs;

  TrustRecord({
    required this.deviceId,
    required this.name,
    required this.fingerprint,
    required this.trusted,
    required this.firstSeenMs,
    required this.lastSeenMs,
  });

  factory TrustRecord.fromJson(Map<String, dynamic> j) => TrustRecord(
    deviceId: j['device_id'] as String? ?? '',
    name: j['name'] as String? ?? '',
    fingerprint: j['fingerprint'] as String? ?? '',
    trusted: j['trusted'] as bool? ?? false,
    firstSeenMs: (j['first_seen_ms'] as num?)?.toInt() ?? 0,
    lastSeenMs: (j['last_seen_ms'] as num?)?.toInt() ?? 0,
  );
}

class NoteItem {
  NoteItem.fromJson(Map<String, dynamic> json) : data = Map.unmodifiable(json);
  final Map<String, dynamic> data;
  String get id => data['id'] as String;
  String get title => data['title'] as String;
  String get body => data['body'] as String? ?? '';
  String get group => data['group'] as String;
  int get order => (data['order'] as num).toInt();
  bool get locked => data['locked'] != null;
  bool get pinned => data['pinned'] == true;
  bool get shielded => data['shielded'] == true;
  bool get deleted => data['deleted'] == true;
}

class TemporaryThread {
  TemporaryThread.fromJson(Map<String, dynamic> json)
    : id = json['id'] as String,
      peerDeviceId = json['peer_device_id'] as String,
      conversationId = json['conversation_id'] as String,
      title = json['title'] as String,
      deleted = json['deleted'] == true,
      hidden = json['hidden'] == true;
  final String id;
  final String peerDeviceId;
  final String conversationId;
  final String title;
  final bool deleted;
  final bool hidden;
}

class MessageItem {
  final String id;
  final String direction; // outgoing / incoming
  final String kind; // text / link
  final String text;
  final String? url;
  final int tsMs;
  final String? delivery;

  MessageItem({
    required this.id,
    required this.direction,
    required this.kind,
    required this.text,
    this.url,
    required this.tsMs,
    this.delivery,
  });

  bool get isOutgoing => direction == 'outgoing';

  factory MessageItem.fromJson(Map<String, dynamic> j) => MessageItem(
    id: j['id'] as String? ?? '',
    direction: j['direction'] as String? ?? 'incoming',
    kind: j['kind'] as String? ?? 'text',
    text: j['text'] as String? ?? '',
    url: j['url'] as String?,
    tsMs: (j['ts_ms'] as num?)?.toInt() ?? 0,
    delivery: j['delivery'] as String?,
  );
}

class TransferItem {
  final String transferId;
  final String peerDeviceId;
  final String? threadId;
  String get conversationId =>
      threadId == null ? peerDeviceId : 'thread:$peerDeviceId:$threadId';
  final String direction; // outgoing / incoming
  final String
  state; // offered/accepted/in_progress/done/failed/canceled/rejected
  final String fileName;
  final int fileSize;
  final int transferred;
  final int speedBps;
  final String? error;
  final int resumeOffset;
  final String? localPath;
  final int tsMs;

  TransferItem({
    required this.transferId,
    required this.peerDeviceId,
    this.threadId,
    required this.direction,
    required this.state,
    required this.fileName,
    required this.fileSize,
    required this.transferred,
    required this.speedBps,
    this.error,
    required this.resumeOffset,
    this.localPath,
    required this.tsMs,
  });

  bool get isOutgoing => direction == 'outgoing';
  bool get isDone => state == 'done';
  bool get isFailed => state == 'failed';
  bool get isCanceled => state == 'canceled';
  bool get isOffered => state == 'offered';
  bool get isInProgress => state == 'in_progress' || state == 'accepted';
  bool get isPaused => state == 'paused';

  double get progress =>
      fileSize <= 0 ? 0 : (transferred / fileSize).clamp(0.0, 1.0);

  String get humanSize {
    final b = fileSize.toDouble();
    if (b >= 1073741824) return '${(b / 1073741824).toStringAsFixed(2)} GB';
    if (b >= 1048576) return '${(b / 1048576).toStringAsFixed(1)} MB';
    if (b >= 1024) return '${(b / 1024).toStringAsFixed(0)} KB';
    return '$fileSize B';
  }

  String get humanSpeed {
    if (speedBps <= 0) return '';
    final b = speedBps.toDouble();
    if (b >= 1048576) return '${(b / 1048576).toStringAsFixed(1)} MB/s';
    if (b >= 1024) return '${(b / 1024).toStringAsFixed(0)} KB/s';
    return '$speedBps B/s';
  }

  factory TransferItem.fromJson(Map<String, dynamic> j) => TransferItem(
    transferId: j['transfer_id'] as String? ?? '',
    peerDeviceId: j['peer_device_id'] as String? ?? '',
    threadId: j['thread_id'] as String?,
    direction: j['direction'] as String? ?? 'incoming',
    state: j['state'] as String? ?? 'offered',
    fileName: j['file_name'] as String? ?? '',
    fileSize: (j['file_size'] as num?)?.toInt() ?? 0,
    transferred: (j['transferred'] as num?)?.toInt() ?? 0,
    speedBps: (j['speed_bps'] as num?)?.toInt() ?? 0,
    error: j['error'] as String?,
    resumeOffset: (j['resume_offset'] as num?)?.toInt() ?? 0,
    localPath: j['local_path'] as String?,
    tsMs: (j['ts_ms'] as num?)?.toInt() ?? 0,
  );
}

class Conversation {
  final String deviceId;
  final String peerName;
  final List<MessageItem> messages;
  final List<TransferItem> transfers;

  Conversation({
    required this.deviceId,
    required this.peerName,
    required this.messages,
    required this.transfers,
  });

  factory Conversation.fromJson(Map<String, dynamic> j) => Conversation(
    deviceId: j['device_id'] as String? ?? '',
    peerName: j['peer_name'] as String? ?? '',
    messages: ((j['messages'] as List?) ?? [])
        .map((e) => MessageItem.fromJson(e as Map<String, dynamic>))
        .toList(),
    transfers: ((j['transfers'] as List?) ?? [])
        .map((e) => TransferItem.fromJson(e as Map<String, dynamic>))
        .toList(),
  );
}
