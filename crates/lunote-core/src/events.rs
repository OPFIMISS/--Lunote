//! 核心事件总线：UI / CLI 只消费事件，不直接访问网络层。

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// 传输方向
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Outgoing,
    Incoming,
}

/// 消息种类
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MsgKind {
    Text,
    Link,
}

/// 传输状态（UI 展示与测试断言共用）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Offered,    // 对方发来文件提议，等待用户确认
    Accepted,   // 已确认/已接受
    InProgress, // 传输中
    Done,       // 完成（含完整性校验通过）
    Failed,     // 失败（可重试/续传）
    Canceled,   // 用户或对端取消
    Rejected,   // 对端拒绝
}

/// 核心事件
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CoreEvent {
    /// 发现新设备（或重启后的新实例）在线
    PeerOnline {
        device_id: String,
        instance_id: String,
        name: String,
        ip: String,
        port: u16,
    },
    /// 设备离线（超时未收到信标）
    PeerOffline {
        device_id: String,
        instance_id: String,
        name: String,
    },
    /// 同一实例的名称更新
    PeerNameChanged {
        device_id: String,
        instance_id: String,
        old_name: String,
        new_name: String,
    },
    /// 可信设备/已知设备身份指纹变化（潜在冒充，必须警告）
    IdentityChanged {
        device_id: String,
        name: String,
        old_fingerprint: String,
        new_fingerprint: String,
    },
    /// 会话已建立（含信任状态）
    PeerConnected {
        device_id: String,
        name: String,
        trusted: bool,
        is_new_device: bool,
        /// 本次连接是否由“同名同 IP 自动信任”自动确认
        auto_trusted: bool,
    },
    /// 会话断开
    PeerDisconnected { device_id: String, reason: String },
    /// 收到消息
    MessageReceived {
        device_id: String,
        message_id: String,
        kind: MsgKind,
        text: String,
        ts_ms: i64,
        from_untrusted: bool,
    },
    /// 消息发送成功（已写入通道）
    MessageSent {
        device_id: String,
        message_id: String,
        kind: MsgKind,
        text: String,
        ts_ms: i64,
    },
    /// 传输状态更新
    TransferUpdate(TransferInfo),
    /// 信任状态变化
    TrustChanged {
        device_id: String,
        name: String,
        trusted: bool,
    },
    /// 本地记录变化（UI 可借此刷新）
    RecordsChanged,
    /// 日志（级别 0=debug 1=info 2=warn 3=error）
    Log { level: u8, msg: String },
}

/// 传输进度信息（事件与查询共用）
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TransferInfo {
    pub transfer_id: String,
    pub peer_device_id: String,
    pub direction: Direction,
    pub state: TransferState,
    pub file_name: String,
    pub file_size: u64,
    pub transferred: u64,
    pub speed_bps: u64,
    /// 失败原因（state=failed 时）
    pub error: Option<String>,
    /// 剩余/已接收偏移（断点续传时 > 0）
    pub resume_offset: u64,
    /// 本端可访问的源文件或最终接收文件路径（本地记录中加密保存）
    #[serde(default)]
    pub local_path: Option<String>,
    pub ts_ms: i64,
}

/// 事件总线（广播：无接收者时丢弃，不影响核心）
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<CoreEvent>,
}

impl EventBus {
    pub fn new() -> (Self, broadcast::Receiver<CoreEvent>) {
        let (tx, rx) = broadcast::channel(4096);
        (Self { tx }, rx)
    }

    pub fn emit(&self, event: CoreEvent) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        let (b, _rx) = Self::new();
        b
    }
}
