//! 会话帧协议与控制消息。
//!
//! 帧格式：`[4B payload 长度 LE][1B kind][payload]`
//! - kind=0：JSON 控制消息（`Control`，含 type 字段）
//! - kind=1：文件分块二进制数据
//!
//! 认证握手（TLS 通道内）：
//! 1. 服务端先发送 32 字节随机 challenge（原始字节，未走帧格式）；
//! 2. 客户端发送 `Hello`（含自己的 challenge + 证书 + 对服务端 challenge 的签名）；
//! 3. 服务端验证后发送 `HelloAck`（含对客户端 challenge 的签名）；
//! 4. 双方各自核验身份指纹（TOFU / 信任库）。

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

pub const FRAME_JSON: u8 = 0;
pub const FRAME_CHUNK: u8 = 1;
pub const CHALLENGE_LEN: usize = 32;
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024; // 控制帧上限 16 MiB

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    /// 客户端 → 服务端：身份声明
    Hello {
        device_id: String,
        name: String,
        instance_id: String,
        challenge: String, // base64 32B，本端随机数
        cert_b64: String,
        sig_b64: String, // 对 challenge(服务端)||device_id||name||instance_id 的签名
        ts_ms: i64,
    },
    /// 服务端 → 客户端：身份确认
    HelloAck {
        device_id: String,
        name: String,
        instance_id: String,
        cert_b64: String,
        sig_b64: String, // 对 challenge(客户端)||device_id||name||instance_id 的签名
        ts_ms: i64,
    },
    Text {
        id: String,
        text: String,
        ts_ms: i64,
    },
    Link {
        id: String,
        url: String,
        title: Option<String>,
        ts_ms: i64,
    },
    FileOffer {
        transfer_id: String,
        name: String,
        size: u64,
        /// 可选：发送端已算出的完整文件 SHA-256
        sha256: Option<String>,
        mtime_ms: Option<i64>,
        /// 文件夹内的相对路径（单文件为 None）
        rel_path: Option<String>,
        /// 断点续传令牌（内容无关的稳定指纹，由发送端生成）
        resume_token: Option<String>,
    },
    FileAccept {
        transfer_id: String,
        /// 从 offset 开始（0 = 从头）
        offset: u64,
        /// 已接收部分的 SHA-256（offset>0 时，供发送端校验后接续）
        partial_sha256: Option<String>,
    },
    FileReject {
        transfer_id: String,
        reason: String,
    },
    /// 发送端发现接收端已收部分与本地不一致：要求接收端清空临时文件从头接收
    FileRestart {
        transfer_id: String,
    },
    ChunkAck {
        transfer_id: String,
        seq: u64,
        offset: u64,
    },
    FileDone {
        transfer_id: String,
        sha256: String,
        size: u64,
    },
    FileCancel {
        transfer_id: String,
        reason: String,
    },
    Ping {
        ts_ms: i64,
    },
    Pong {
        ts_ms: i64,
    },
    Bye {
        reason: String,
    },
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub kind: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn json(control: &Control) -> Result<Self, serde_json::Error> {
        Ok(Self {
            kind: FRAME_JSON,
            payload: serde_json::to_vec(control)?,
        })
    }

    pub fn chunk(payload: Vec<u8>) -> Self {
        Self {
            kind: FRAME_CHUNK,
            payload,
        }
    }

    pub fn as_control(&self) -> Option<Control> {
        if self.kind == FRAME_JSON {
            serde_json::from_slice(&self.payload).ok()
        } else {
            None
        }
    }
}

pub fn encode_frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

pub async fn write_frame(
    w: &mut (impl AsyncWrite + Unpin),
    kind: u8,
    payload: &[u8],
) -> io::Result<()> {
    let frame = encode_frame(kind, payload);
    w.write_all(&frame).await
}

pub async fn read_frame(r: &mut (impl AsyncRead + Unpin)) -> io::Result<Frame> {
    let mut header = [0u8; 5];
    r.read_exact(&mut header).await?;
    let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let kind = header[4];
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("帧过长: {} 字节", len),
        ));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload).await?;
    Ok(Frame { kind, payload })
}

/// 认证签名输入：challenge || "|" || device_id || "|" || name || "|" || instance_id
pub fn signed_data(challenge_b64: &str, device_id: &str, name: &str, instance_id: &str) -> Vec<u8> {
    let mut data = Vec::with_capacity(256);
    data.extend_from_slice(challenge_b64.as_bytes());
    data.push(b'|');
    data.extend_from_slice(device_id.as_bytes());
    data.push(b'|');
    data.extend_from_slice(name.as_bytes());
    data.push(b'|');
    data.extend_from_slice(instance_id.as_bytes());
    data
}

pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_roundtrip() {
        let c = Control::Text {
            id: "id-1".into(),
            text: "你好，月笺".into(),
            ts_ms: 123,
        };
        let f = Frame::json(&c).unwrap();
        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, f.kind, &f.payload).await.unwrap();
        let mut cur = std::io::Cursor::new(buf);
        let back = read_frame(&mut cur).await.unwrap();
        match back.as_control().unwrap() {
            Control::Text { id, text, ts_ms } => {
                assert_eq!(id, "id-1");
                assert_eq!(text, "你好，月笺");
                assert_eq!(ts_ms, 123);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[tokio::test]
    async fn oversized_frame_rejected() {
        let mut big = Vec::new();
        big.extend_from_slice(&(MAX_FRAME_LEN as u32 + 1).to_le_bytes());
        big.push(0);
        let mut cur = std::io::Cursor::new(big);
        assert!(read_frame(&mut cur).await.is_err());
    }
}
