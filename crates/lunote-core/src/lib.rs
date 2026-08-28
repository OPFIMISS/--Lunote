//! 月笺 Lunote 核心库
//!
//! 模块划分（按依赖顺序）：
//! - `discovery`  — 局域网自动发现（UDP 组播 + 广播，LUNOTE1 协议，带版本字段）
//! - `identity`   — 持久化设备身份（Ed25519 长期密钥 + 自签名证书）
//! - `trust`      — 首次信任与可信设备管理（TOFU + 用户确认）
//! - `session`    — TLS 1.3 双向认证会话与帧协议
//! - `messages`   — 文字 / 链接 / 文件控制消息
//! - `transfer`   — 流式文件 / 文件夹传输（完整性 / 取消 / 断点续传）
//! - `store`      — 本地加密记录（AES-256-GCM）与导出 / 导入（Argon2id）
//! - `events`     — 核心事件总线（UI 只消费事件，不直接摸网络层）
//! - `runtime`    — Runtime 聚合：启动 / 停止 / 后台节流 / 平台接口

pub mod discovery;
pub mod events;
pub mod identity;
pub mod messages;
pub mod platform;
pub mod runtime;
pub mod session;
pub mod store;
pub mod transfer;
pub mod trust;

pub use runtime::{Runtime, RuntimeConfig};
