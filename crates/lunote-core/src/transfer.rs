//! 流式文件 / 文件夹传输：完整性校验、取消、失败重试、断点续传。
//!
//! 设计：
//! - 流式：256 KiB 块，双端异步读写，不整文件载入内存；
//! - 完整性：收发双方独立增量 SHA-256，`FileDone` 时比对；
//! - 断点续传：接收端保留 `.part` 临时文件；重连后新 `FileOffer` 携带
//!   resume_token，接收端回 `FileAccept{offset, partial_sha256}`，
//!   发送端先校验部分哈希再接续；不一致则 `FileRestart` 从头开始；
//! - 取消：任一侧可取消，接收端删除临时文件；
//! - 失败（连接断开）：保留 `.part`，状态标为“可续传”；
//! - 安全：文件名/相对路径净化，拒绝绝对路径与 `..`（防目录穿越）；
//!   发送前检查剩余空间；接收完成原子改名（临时文件 + rename）；
//! - 信任门禁：未信任设备发来的文件提议直接拒绝；发送文件要求已信任。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::Read as _;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::events::{CoreEvent, Direction, EventBus, TransferInfo, TransferState};
use crate::messages::{new_id, Control};
use crate::platform::{free_space, sanitize_file_name, sanitize_rel_path};
use crate::session::SessionManager;
use crate::store::Store;
use crate::trust::TrustStore;

pub const CHUNK_SIZE: usize = 256 * 1024;
pub const WINDOW_CHUNKS: u64 = 16; // 未确认窗口上限（4 MiB）
pub const ACK_EVERY: u64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictPolicy {
    Rename,
    Overwrite,
    Skip,
}
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(120);
const ACK_TIMEOUT: Duration = Duration::from_secs(30);
const CHUNK_HEADER_LEN: usize = 36 + 8 + 8; // transfer_id + offset + seq

#[derive(Clone)]
pub struct SendFile {
    pub path: PathBuf,
    /// 文件夹内相对路径（单文件为空）
    pub rel_parts: Vec<String>,
}

struct IncomingState {
    transfer_id: String,
    peer: String,
    file_name: String,
    rel_parts: Vec<String>,
    size: u64,
    sha256_expected: Option<String>,
    resume_token: Option<String>,
    state: TransferState,
    dest_dir: Option<PathBuf>,
    temp_path: Option<PathBuf>,
    local_path: Option<PathBuf>,
    file: Option<tokio::fs::File>,
    hasher: Option<Sha256>,
    received: u64,
    partial_hash: Option<String>,
    error: Option<String>,
    chunk_seq: u64,
    last_emit: Option<Instant>,
    last_emit_bytes: u64,
    speed_bps: u64,
    ts_ms: i64,
}

#[derive(Clone)]
struct OutgoingState {
    transfer_id: String,
    peer: String,
    file_name: String,
    #[allow(dead_code)]
    rel_parts: Vec<String>,
    path: PathBuf,
    size: u64,
    sent: u64,
    state: TransferState,
    error: Option<String>,
    resume_offset: u64,
    chunk_seq: u64,
    last_emit: Option<Instant>,
    last_emit_bytes: u64,
    speed_bps: u64,
    ts_ms: i64,
}

enum OutgoingMsg {
    Control(Control),
    Disconnected(String),
}

pub struct TransferManager {
    bus: EventBus,
    store: Arc<Store>,
    trust: Arc<Mutex<TrustStore>>,
    downloads_dir: PathBuf,
    session: OnceLock<Weak<SessionManager>>,
    incoming: Mutex<HashMap<String, IncomingState>>,
    outgoing: Mutex<HashMap<String, OutgoingState>>,
    outgoing_tx: Mutex<HashMap<String, mpsc::Sender<OutgoingMsg>>>,
    outgoing_paused: Mutex<HashMap<String, bool>>,
    conflict_policy: Mutex<ConflictPolicy>,
}

impl TransferManager {
    pub fn new(
        bus: EventBus,
        store: Arc<Store>,
        trust: Arc<Mutex<TrustStore>>,
        downloads_dir: PathBuf,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&downloads_dir)
            .with_context(|| format!("创建接收目录失败: {}", downloads_dir.display()))?;
        Ok(Arc::new(Self {
            bus,
            store,
            trust,
            downloads_dir,
            session: OnceLock::new(),
            incoming: Mutex::new(HashMap::new()),
            outgoing: Mutex::new(HashMap::new()),
            outgoing_tx: Mutex::new(HashMap::new()),
            outgoing_paused: Mutex::new(HashMap::new()),
            conflict_policy: Mutex::new(ConflictPolicy::Rename),
        }))
    }

    pub fn set_conflict_policy(&self, policy: ConflictPolicy) {
        *self.conflict_policy.lock().unwrap() = policy;
    }

    pub fn conflict_policy(&self) -> ConflictPolicy {
        *self.conflict_policy.lock().unwrap()
    }

    pub fn set_session(&self, session: Arc<SessionManager>) {
        let _ = self.session.set(Arc::downgrade(&session));
    }

    fn session(&self) -> Result<Arc<SessionManager>> {
        self.session
            .get()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| anyhow!("会话层未就绪"))
    }

    async fn send_control(&self, peer: &str, control: Control) -> Result<()> {
        self.session()?.send_control(peer, control).await
    }

    async fn send_chunk(&self, peer: &str, payload: Vec<u8>) -> Result<()> {
        self.session()?.send_chunk(peer, payload).await
    }

    // ---------- 对外 API（Runtime 调用） ----------

    /// 发送文件（文件夹时逐文件发送，携带相对路径）
    pub async fn send_files(
        self: &Arc<Self>,
        peer: &str,
        files: Vec<SendFile>,
    ) -> Result<Vec<String>> {
        let trusted = self.trust.lock().unwrap().is_trusted(peer);
        if !trusted {
            bail!("设备未信任，不能发送文件（请先在信任列表确认）");
        }
        let session = self.session()?;
        if !session.is_connected(peer) {
            session.connect_to(peer).await?;
        }
        let mut ids = Vec::new();
        for f in files {
            let id = self.spawn_outgoing(peer, f).await?;
            ids.push(id);
        }
        Ok(ids)
    }

    pub async fn accept_transfer(&self, transfer_id: &str, dest_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dest_dir)
            .with_context(|| format!("创建接收目录失败: {}", dest_dir.display()))?;
        let st = {
            let mut map = self.incoming.lock().unwrap();
            let st = map
                .get_mut(transfer_id)
                .ok_or_else(|| anyhow!("传输不存在或已结束: {}", transfer_id))?;
            if st.state != TransferState::Offered {
                bail!("传输状态不是待确认（当前 {:?}）", st.state);
            }
            st.dest_dir = Some(dest_dir.to_path_buf());
            st.state = TransferState::Accepted;
            st.clone_for_send()
        };
        // 剩余空间检查
        let avail = free_space(dest_dir)?;
        if avail < st.size {
            bail!(
                "接收目录剩余空间不足（需要 {} 字节，剩余 {} 字节）",
                st.size,
                avail
            );
        }
        let partial = if st.received > 0 {
            st.partial_hash.clone()
        } else {
            None
        };
        self.send_control(
            &st.peer,
            Control::FileAccept {
                transfer_id: transfer_id.to_string(),
                offset: st.received,
                partial_sha256: partial,
            },
        )
        .await?;
        self.emit_update(st.into_info(Some("已确认".into())));
        Ok(())
    }

    pub async fn reject_transfer(&self, transfer_id: &str, reason: &str) -> Result<()> {
        let st = {
            let mut map = self.incoming.lock().unwrap();
            let st = map
                .get_mut(transfer_id)
                .ok_or_else(|| anyhow!("传输不存在: {}", transfer_id))?;
            if st.state != TransferState::Offered {
                bail!("传输状态不是待确认");
            }
            st.state = TransferState::Rejected;
            st.clone_for_send()
        };
        self.send_control(
            &st.peer,
            Control::FileReject {
                transfer_id: transfer_id.to_string(),
                reason: reason.to_string(),
            },
        )
        .await?;
        self.emit_update(st.into_info(Some(reason.to_string())));
        Ok(())
    }

    pub async fn cancel_transfer(&self, transfer_id: &str) -> Result<()> {
        // 收方向：删除临时文件并通知对端
        let incoming_cancel: Option<(String, TransferInfo)> = {
            let mut map = self.incoming.lock().unwrap();
            if let Some(st) = map.get_mut(transfer_id) {
                if matches!(
                    st.state,
                    TransferState::Done | TransferState::Canceled | TransferState::Rejected
                ) {
                    return Err(anyhow!("传输已结束，无法取消"));
                }
                let peer = st.peer.clone();
                st.state = TransferState::Canceled;
                st.file = None;
                if let Some(p) = &st.temp_path {
                    let _ = std::fs::remove_file(p);
                }
                if let Some(token) = &st.resume_token {
                    self.remove_resume_meta(token);
                }
                let info = st.clone_for_send().into_info(Some("用户取消".into()));
                Some((peer, info))
            } else {
                None
            }
        };
        if let Some((peer, info)) = incoming_cancel {
            self.send_control(
                &peer,
                Control::FileCancel {
                    transfer_id: transfer_id.to_string(),
                    reason: "用户取消".into(),
                },
            )
            .await?;
            self.emit_update(info);
            return Ok(());
        }
        // 发方向：通知发送任务
        let tx = self
            .outgoing_tx
            .lock()
            .unwrap()
            .get(transfer_id)
            .cloned()
            .ok_or_else(|| anyhow!("传输不存在: {}", transfer_id))?;
        tx.send(OutgoingMsg::Control(Control::FileCancel {
            transfer_id: transfer_id.to_string(),
            reason: "用户取消".into(),
        }))
        .await
        .map_err(|_| anyhow!("发送任务已结束"))?;
        Ok(())
    }

    pub async fn pause_transfer(&self, transfer_id: &str) -> Result<()> {
        if !self.outgoing_tx.lock().unwrap().contains_key(transfer_id) {
            bail!("传输不存在或已结束");
        }
        self.outgoing_paused
            .lock()
            .unwrap()
            .insert(transfer_id.to_string(), true);
        Ok(())
    }

    pub async fn resume_transfer(&self, transfer_id: &str) -> Result<()> {
        if !self.outgoing_tx.lock().unwrap().contains_key(transfer_id) {
            bail!("传输不存在或已结束");
        }
        self.outgoing_paused
            .lock()
            .unwrap()
            .insert(transfer_id.to_string(), false);
        Ok(())
    }

    /// 当前所有传输快照（UI / CLI）
    pub fn list(&self) -> Vec<TransferInfo> {
        let mut out = Vec::new();
        {
            let map = self.incoming.lock().unwrap();
            for st in map.values() {
                out.push(st.clone_for_send().into_info(None));
            }
        }
        {
            let map = self.outgoing.lock().unwrap();
            for st in map.values() {
                out.push(st.into_info());
            }
        }
        out.sort_by(|a, b| b.ts_ms.cmp(&a.ts_ms));
        out
    }

    // ---------- 会话分发入口 ----------

    pub async fn on_control(&self, peer: &str, control: Control) {
        let result = match control {
            Control::FileOffer { .. } => self.on_offer(peer, control).await,
            Control::FileAccept { .. } => self.forward_outgoing(peer, control).await,
            Control::FileReject { .. } => self.forward_outgoing(peer, control).await,
            Control::FileRestart { .. } => self.on_restart(peer, control).await,
            Control::ChunkAck { .. } => self.forward_outgoing(peer, control).await,
            Control::FileDone { .. } => {
                if self
                    .incoming
                    .lock()
                    .unwrap()
                    .contains_key(transfer_id_of(&control))
                {
                    self.on_done(peer, control).await
                } else {
                    self.forward_outgoing(peer, control).await
                }
            }
            Control::FileCancel { .. } => {
                if self
                    .incoming
                    .lock()
                    .unwrap()
                    .contains_key(transfer_id_of(&control))
                {
                    self.on_peer_cancel(peer, control).await
                } else {
                    self.forward_outgoing(peer, control).await
                }
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            tracing::warn!("传输控制处理失败 peer={} err={}", peer, e);
        }
    }

    pub async fn on_chunk(&self, peer: &str, payload: Vec<u8>) {
        if let Err(e) = self.receive_chunk(peer, payload).await {
            tracing::warn!("接收分块失败 peer={} err={}", peer, e);
        }
    }

    pub async fn on_peer_disconnected(&self, peer: &str, reason: &str) {
        tracing::warn!("对端断开，标记可续传 peer={} reason={}", peer, reason);
        // 收方向：保留 .part，标记可续传
        {
            let mut incoming = self.incoming.lock().unwrap();
            for st in incoming.values_mut() {
                if st.peer != peer {
                    continue;
                }
                if matches!(
                    st.state,
                    TransferState::InProgress | TransferState::Accepted | TransferState::Offered
                ) {
                    st.state = TransferState::Failed;
                    st.error = Some(format!("连接断开（可续传）: {}", reason));
                    st.file = None;
                    self.emit_update(st.clone_for_send().into_info(st.error.clone()));
                }
            }
        }
        // 发方向：通知任务
        let txs: Vec<(String, mpsc::Sender<OutgoingMsg>)> = {
            let map = self.outgoing_tx.lock().unwrap();
            let outgoing = self.outgoing.lock().unwrap();
            let mut v = Vec::new();
            for (id, tx) in map.iter() {
                if let Some(st) = outgoing.get(id) {
                    if st.peer == peer {
                        v.push((id.clone(), tx.clone()));
                    }
                }
            }
            v
        };
        for (_, tx) in txs {
            let _ = tx.try_send(OutgoingMsg::Disconnected(reason.to_string()));
        }
    }

    // ---------- 收方向 ----------

    async fn on_offer(&self, peer: &str, control: Control) -> Result<()> {
        let Control::FileOffer {
            transfer_id,
            name,
            size,
            sha256,
            mtime_ms: _,
            rel_path,
            resume_token,
        } = control
        else {
            bail!("FileOffer 解析错误");
        };
        // 防御：transfer_id 必须是 36 字符 UUID，否则拒绝。
        // 它会被拼接进临时文件路径（.{id}.lunote-part），不校验=目录穿越。
        if !is_uuid(&transfer_id) {
            bail!("非法传输 ID（非 UUID）: {}", transfer_id);
        }
        let trusted = self.trust.lock().unwrap().is_trusted(peer);
        if !trusted {
            self.send_control(
                peer,
                Control::FileReject {
                    transfer_id: transfer_id.clone(),
                    reason: "未信任设备不能发送文件".into(),
                },
            )
            .await?;
            let info = TransferInfo {
                transfer_id: transfer_id.clone(),
                peer_device_id: peer.to_string(),
                direction: Direction::Incoming,
                state: TransferState::Rejected,
                file_name: sanitize_file_name(&name),
                file_size: size,
                transferred: 0,
                speed_bps: 0,
                error: Some("未信任设备不能发送文件".into()),
                resume_offset: 0,
                local_path: None,
                ts_ms: now_ms(),
            };
            self.emit_update(info);
            return Ok(());
        }
        let safe_name = sanitize_file_name(&name);
        let rel_parts = match rel_path {
            Some(rp) => {
                let mut parts =
                    sanitize_rel_path(&rp).ok_or_else(|| anyhow!("非法相对路径: {}", rp))?;
                // 防御：若末位组件与文件名相同则去掉（只保留目录）
                if parts.last().map(|s| s == &safe_name).unwrap_or(false) {
                    parts.pop();
                }
                parts
            }
            None => vec![],
        };
        // 断点续传：检查已有 .part
        let (received, partial_hash, existing_part) =
            self.find_partial(&safe_name, &rel_parts, &resume_token)?;
        let st = IncomingState {
            transfer_id: transfer_id.clone(),
            peer: peer.to_string(),
            file_name: safe_name,
            rel_parts,
            size,
            sha256_expected: sha256,
            resume_token,
            state: TransferState::Offered,
            dest_dir: None,
            temp_path: existing_part,
            local_path: None,
            file: None,
            hasher: None,
            received,
            partial_hash,
            error: None,
            chunk_seq: 0,
            last_emit: None,
            last_emit_bytes: 0,
            speed_bps: 0,
            ts_ms: now_ms(),
        };
        self.incoming
            .lock()
            .unwrap()
            .insert(transfer_id.clone(), st);
        self.emit_update(
            self.incoming
                .lock()
                .unwrap()
                .get(&transfer_id)
                .unwrap()
                .clone_for_send()
                .into_info(None),
        );
        Ok(())
    }

    /// 查找可续传的 .part 文件（按 resume_token 映射文件）
    /// 返回 (已收字节数, 已收部分哈希, 已有 .part 路径)
    fn find_partial(
        &self,
        name: &str,
        rel_parts: &[String],
        resume_token: &Option<String>,
    ) -> Result<(u64, Option<String>, Option<PathBuf>)> {
        let none = Ok((0, None, None));
        let Some(token) = resume_token else {
            return none;
        };
        let token_dir = self.downloads_dir.join(".lunote-resume");
        if !token_dir.is_dir() {
            return none;
        }
        let _ = name;
        let _ = rel_parts;
        // 令牌文件名 = sha256(token) 前 32 位，内容存映射
        let key = crate::identity::sha256_hex(token.as_bytes());
        let meta_path = token_dir.join(format!("{}.json", key));
        if !meta_path.exists() {
            return none;
        }
        let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
        let Some(part_path) = meta.get("part").and_then(|v| v.as_str()).map(PathBuf::from) else {
            return none;
        };
        if !part_path.exists() {
            return none;
        }
        let size = std::fs::metadata(&part_path)?.len();
        // 计算已收部分哈希（用于发送端校验）
        let mut hasher = Sha256::new();
        let mut f = std::fs::File::open(&part_path)?;
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let hash = hex(&hasher.finalize());
        Ok((size, Some(hash), Some(part_path)))
    }

    async fn on_restart(&self, peer: &str, control: Control) -> Result<()> {
        let Control::FileRestart { transfer_id } = control else {
            bail!("FileRestart 解析错误");
        };
        let mut map = self.incoming.lock().unwrap();
        let st = map
            .get_mut(&transfer_id)
            .ok_or_else(|| anyhow!("传输不存在: {}", transfer_id))?;
        if st.peer != peer {
            bail!("传输不属于该对端");
        }
        st.received = 0;
        st.partial_hash = None;
        st.file = None;
        if let Some(token) = &st.resume_token {
            let key = crate::identity::sha256_hex(token.as_bytes());
            let _ = std::fs::remove_file(
                self.downloads_dir
                    .join(".lunote-resume")
                    .join(format!("{}.json", key)),
            );
        }
        if let Some(p) = &st.temp_path {
            let _ = std::fs::remove_file(p);
        }
        st.temp_path = None;
        st.state = TransferState::InProgress;
        st.chunk_seq = 0;
        st.hasher = Some(Sha256::new());
        Ok(())
    }

    pub async fn receive_chunk(&self, peer: &str, payload: Vec<u8>) -> Result<()> {
        if payload.len() < CHUNK_HEADER_LEN {
            bail!("分块过短");
        }
        let transfer_id = String::from_utf8_lossy(&payload[..36]).to_string();
        let offset = u64::from_le_bytes(payload[36..44].try_into().unwrap());
        let seq = u64::from_le_bytes(payload[44..52].try_into().unwrap());
        let data = &payload[52..];
        tracing::debug!(
            "[in {}] 收到分块 seq={} offset={} len={}",
            transfer_id,
            seq,
            offset,
            data.len()
        );

        // 取出状态（锁内只做同步操作）
        let mut st = {
            let mut map = self.incoming.lock().unwrap();
            let Some(st) = map.remove(&transfer_id) else {
                bail!("未知传输 {}", transfer_id);
            };
            let st = st;
            if st.peer != peer {
                map.insert(transfer_id.clone(), st);
                bail!("分块来自错误对端");
            }
            if st.state == TransferState::Offered {
                map.insert(transfer_id.clone(), st);
                bail!("尚未确认接收，收到分块（协议错误）");
            }
            st
        };

        // 首次分块：打开临时文件（异步 IO 不持锁）
        if st.file.is_none()
            && (st.state == TransferState::Accepted || st.state == TransferState::InProgress)
        {
            let dir = st
                .dest_dir
                .clone()
                .unwrap_or_else(|| self.downloads_dir.clone());
            let temp_path = st
                .temp_path
                .clone()
                .unwrap_or_else(|| dir.join(format!(".{}.lunote-part", transfer_id)));
            st.temp_path = Some(temp_path.clone());
            // 写续传元数据（供断线后重连续传）
            if let Some(token) = &st.resume_token {
                let meta = serde_json::json!({
                    "part": temp_path.to_string_lossy(),
                    "name": st.file_name,
                    "size": st.size,
                });
                let key = crate::identity::sha256_hex(token.as_bytes());
                let token_dir = self.downloads_dir.join(".lunote-resume");
                let _ = std::fs::create_dir_all(&token_dir);
                let _ = std::fs::write(
                    token_dir.join(format!("{}.json", key)),
                    serde_json::to_string(&meta).unwrap_or_default(),
                );
            }
            let mut hasher = Sha256::new();
            if st.received > 0 {
                // 恢复续传时，最终完整性校验必须包含已落盘的前缀。
                // partial_hash 只用于发送端确认 offset，不能替代可继续 update 的状态。
                let mut existing = match tokio::fs::File::open(&temp_path).await {
                    Ok(file) => file,
                    Err(e) => {
                        st.state = TransferState::Failed;
                        st.error = Some(format!("读取续传分片失败: {}", e));
                        let info = st.clone_for_send().into_info(st.error.clone());
                        self.reinsert_incoming(&transfer_id, st);
                        self.emit_update(info);
                        return Err(anyhow!("读取续传分片失败: {}", e));
                    }
                };
                let mut remaining = st.received;
                let mut prefix = vec![0u8; CHUNK_SIZE];
                while remaining > 0 {
                    let want = remaining.min(prefix.len() as u64) as usize;
                    match existing.read(&mut prefix[..want]).await {
                        Ok(0) => {
                            st.state = TransferState::Failed;
                            st.error = Some("续传分片短于记录偏移".into());
                            let info = st.clone_for_send().into_info(st.error.clone());
                            self.reinsert_incoming(&transfer_id, st);
                            self.emit_update(info);
                            return Err(anyhow!("续传分片短于记录偏移"));
                        }
                        Ok(n) => {
                            hasher.update(&prefix[..n]);
                            remaining -= n as u64;
                        }
                        Err(e) => {
                            st.state = TransferState::Failed;
                            st.error = Some(format!("读取续传分片失败: {}", e));
                            let info = st.clone_for_send().into_info(st.error.clone());
                            self.reinsert_incoming(&transfer_id, st);
                            self.emit_update(info);
                            return Err(anyhow!("读取续传分片失败: {}", e));
                        }
                    }
                }
            }
            st.hasher = Some(hasher);
            let opened = tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(st.received == 0)
                .open(&temp_path)
                .await;
            let mut f = match opened {
                Ok(f) => f,
                Err(e) => {
                    st.state = TransferState::Failed;
                    st.error = Some(format!("打开临时文件失败: {}", e));
                    let info = st.clone_for_send().into_info(st.error.clone());
                    self.reinsert_incoming(&transfer_id, st);
                    self.emit_update(info);
                    return Err(anyhow!("打开临时文件失败: {}", e));
                }
            };
            if st.received > 0 {
                if let Err(e) = f.seek(std::io::SeekFrom::Start(st.received)).await {
                    st.state = TransferState::Failed;
                    st.error = Some(format!("定位续传偏移失败: {}", e));
                    let info = st.clone_for_send().into_info(st.error.clone());
                    self.reinsert_incoming(&transfer_id, st);
                    self.emit_update(info);
                    return Err(anyhow!("定位续传偏移失败: {}", e));
                }
            }
            st.file = Some(f);
            st.state = TransferState::InProgress;
            self.emit_update(st.clone_for_send().into_info(None));
        }

        if offset != st.received {
            tracing::warn!(
                "接收分块乱序 transfer={} 期望={} 收到={}",
                transfer_id,
                st.received,
                offset
            );
            st.state = TransferState::Failed;
            st.error = Some(format!("分块乱序（期望 {}，收到 {}）", st.received, offset));
            let info = st.clone_for_send().into_info(st.error.clone());
            self.reinsert_incoming(&transfer_id, st);
            self.emit_update(info);
            bail!("分块乱序");
        }
        if st.received + data.len() as u64 > st.size {
            st.state = TransferState::Failed;
            st.error = Some("收到的数据超过声明大小".into());
            let info = st.clone_for_send().into_info(st.error.clone());
            self.reinsert_incoming(&transfer_id, st);
            self.emit_update(info);
            bail!("数据超过声明大小");
        }
        {
            let write_res = {
                let file = st.file.as_mut().ok_or_else(|| anyhow!("临时文件未打开"))?;
                file.write_all(data).await
            };
            if let Err(e) = write_res {
                // 保持状态在表中：写失败也标记失败并继续维护，避免传输从 UI 消失
                let info = st
                    .clone_for_send()
                    .into_info(Some(format!("写入临时文件失败: {}", e)));
                self.reinsert_incoming(&transfer_id, st);
                self.emit_update(info);
                bail!("写入临时文件失败: {}", e);
            }
        }
        if let Some(h) = st.hasher.as_mut() {
            h.update(data);
        }
        st.received += data.len() as u64;
        st.chunk_seq = seq;
        let need_ack = st.chunk_seq % ACK_EVERY == 0 || st.received == st.size;
        let ack_seq = st.chunk_seq;
        let ack_offset = st.received;
        let state = st.clone_for_send();
        self.reinsert_incoming(&transfer_id, st);

        if need_ack {
            tracing::debug!(
                "入站 ack transfer={} seq={} offset={}",
                transfer_id,
                ack_seq,
                ack_offset
            );
            self.send_control(
                peer,
                Control::ChunkAck {
                    transfer_id: transfer_id.clone(),
                    seq: ack_seq,
                    offset: ack_offset,
                },
            )
            .await?;
        }
        self.throttled_emit(&transfer_id, state, Direction::Incoming)
            .await;
        Ok(())
    }

    fn reinsert_incoming(&self, transfer_id: &str, st: IncomingState) {
        self.incoming
            .lock()
            .unwrap()
            .insert(transfer_id.to_string(), st);
    }

    fn remove_resume_meta(&self, token: &str) {
        let key = crate::identity::sha256_hex(token.as_bytes());
        let _ = std::fs::remove_file(
            self.downloads_dir
                .join(".lunote-resume")
                .join(format!("{}.json", key)),
        );
    }

    async fn on_done(&self, peer: &str, control: Control) -> Result<()> {
        let Control::FileDone {
            transfer_id,
            sha256,
            size,
        } = control
        else {
            bail!("FileDone 解析错误");
        };
        let mut st = {
            let mut map = self.incoming.lock().unwrap();
            let Some(st) = map.remove(&transfer_id) else {
                bail!("传输不存在: {}", transfer_id);
            };
            let st = st;
            if st.peer != peer {
                map.insert(transfer_id.clone(), st);
                bail!("传输不属于该对端");
            }
            st
        };
        // 计算本地哈希
        let local_hash = st
            .hasher
            .take()
            .map(|h| hex(&h.finalize()))
            .unwrap_or_default();
        let ok_size = st.received == size && st.received == st.size;
        let ok_hash = local_hash == sha256
            && st
                .sha256_expected
                .as_ref()
                .map(|e| e == &sha256)
                .unwrap_or(true);
        if !ok_size || !ok_hash {
            let reason = format!(
                "完整性校验失败（size_ok={} hash_ok={} local={} remote={}）",
                st.received == size,
                local_hash == sha256,
                local_hash,
                sha256
            );
            st.state = TransferState::Failed;
            st.error = Some(reason.clone());
            st.file = None;
            if let Some(p) = &st.temp_path {
                let _ = std::fs::remove_file(p);
            }
            let info = st.clone_for_send().into_info(Some(reason.clone()));
            self.reinsert_incoming(&transfer_id, st);
            self.send_control(
                peer,
                Control::FileCancel {
                    transfer_id: transfer_id.clone(),
                    reason,
                },
            )
            .await?;
            self.emit_update(info);
            return Ok(());
        }
        // 落盘：flush + 原子改名（异步 IO 不持锁）
        if let Some(mut f) = st.file.take() {
            if let Err(e) = f.flush().await {
                st.state = TransferState::Failed;
                st.error = Some(format!("写入完成前落盘失败: {}", e));
                let info = st.clone_for_send().into_info(st.error.clone());
                self.reinsert_incoming(&transfer_id, st);
                self.emit_update(info);
                return Err(anyhow!("写入完成前落盘失败: {}", e));
            }
        }
        let finalize = (|| -> Result<PathBuf> {
            let dir = st
                .dest_dir
                .clone()
                .unwrap_or_else(|| self.downloads_dir.clone());
            let base = dir
                .join(st.rel_parts.iter().fold(PathBuf::new(), |mut p, part| {
                    p.push(part);
                    p
                }))
                .join(&st.file_name);
            let final_path = match self.conflict_policy() {
                ConflictPolicy::Rename => unique_path(&dir, &st.rel_parts, &st.file_name),
                ConflictPolicy::Overwrite => base,
                ConflictPolicy::Skip if base.exists() => return Err(anyhow!("目标文件已存在")),
                ConflictPolicy::Skip => base,
            };
            if let Some(parent) = final_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let temp_path = match st.temp_path.clone() {
                Some(p) => p,
                // 空文件（0 字节）没有分块，也就没有临时文件：直接创建空目标
                None if st.size == 0 => {
                    std::fs::write(&final_path, b"")?;
                    return Ok(final_path);
                }
                None => return Err(anyhow!("临时文件路径缺失")),
            };
            if matches!(self.conflict_policy(), ConflictPolicy::Overwrite) && final_path.exists() {
                std::fs::remove_file(&final_path).context("覆盖已有文件失败")?;
            }
            std::fs::rename(&temp_path, &final_path)?;
            Ok(final_path)
        })();
        let final_path = match finalize {
            Ok(p) => p,
            Err(e) => {
                st.state = TransferState::Failed;
                st.error = Some(format!("落盘改名失败: {}", e));
                let info = st.clone_for_send().into_info(st.error.clone());
                self.reinsert_incoming(&transfer_id, st);
                self.emit_update(info);
                return Err(e);
            }
        };
        // 清理续传元数据
        if let Some(token) = &st.resume_token {
            let key = crate::identity::sha256_hex(token.as_bytes());
            let _ = std::fs::remove_file(
                self.downloads_dir
                    .join(".lunote-resume")
                    .join(format!("{}.json", key)),
            );
        }
        st.state = TransferState::Done;
        st.temp_path = None;
        st.local_path = Some(final_path.clone());
        st.file_name = final_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(st.file_name.clone());
        let info = st.clone_for_send().into_info(None);
        self.reinsert_incoming(&transfer_id, st);
        // 回执
        self.send_control(
            peer,
            Control::FileDone {
                transfer_id: transfer_id.clone(),
                sha256,
                size,
            },
        )
        .await?;
        self.emit_update(info);
        Ok(())
    }

    async fn on_peer_cancel(&self, peer: &str, control: Control) -> Result<()> {
        let Control::FileCancel {
            transfer_id,
            reason,
        } = control
        else {
            bail!("FileCancel 解析错误");
        };
        let mut map = self.incoming.lock().unwrap();
        let mut st = map
            .remove(&transfer_id)
            .ok_or_else(|| anyhow!("传输不存在: {}", transfer_id))?;
        if st.peer != peer {
            map.insert(transfer_id, st);
            bail!("传输不属于该对端");
        }
        st.state = TransferState::Canceled;
        st.error = Some(reason.clone());
        st.file = None;
        if let Some(p) = &st.temp_path {
            let _ = std::fs::remove_file(p);
        }
        if let Some(token) = &st.resume_token {
            self.remove_resume_meta(token);
        }
        let info = st.clone_for_send().into_info(Some(reason));
        map.insert(transfer_id, st);
        self.emit_update(info);
        Ok(())
    }

    // ---------- 发方向 ----------

    async fn spawn_outgoing(self: &Arc<Self>, peer: &str, file: SendFile) -> Result<String> {
        let meta = tokio::fs::metadata(&file.path).await?;
        if !meta.is_file() {
            bail!("不是普通文件: {}", file.path.display());
        }
        let file_name = file
            .path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unnamed".into());
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);
        let resume_token = {
            let mut h = Sha256::new();
            h.update(peer.as_bytes());
            h.update(b"|");
            for p in &file.rel_parts {
                h.update(p.as_bytes());
                h.update(b"/");
            }
            h.update(file_name.as_bytes());
            h.update(b"|");
            h.update(meta.len().to_string().as_bytes());
            if let Some(m) = mtime {
                h.update(b"|");
                h.update(m.to_string().as_bytes());
            }
            hex(&h.finalize())
        };
        let transfer_id = new_id();
        let (tx, rx) = mpsc::channel::<OutgoingMsg>(64);
        self.outgoing_tx
            .lock()
            .unwrap()
            .insert(transfer_id.clone(), tx);
        self.outgoing.lock().unwrap().insert(
            transfer_id.clone(),
            OutgoingState {
                transfer_id: transfer_id.clone(),
                peer: peer.to_string(),
                file_name: file_name.clone(),
                rel_parts: file.rel_parts.clone(),
                path: file.path.clone(),
                size: meta.len(),
                sent: 0,
                state: TransferState::Offered,
                error: None,
                resume_offset: 0,
                chunk_seq: 0,
                last_emit: None,
                last_emit_bytes: 0,
                speed_bps: 0,
                ts_ms: now_ms(),
            },
        );
        let this = self.clone();
        let peer = peer.to_string();
        let tid = transfer_id.clone();
        tokio::spawn(async move {
            this.outgoing_task(
                peer,
                file,
                file_name,
                tid,
                meta.len(),
                mtime,
                resume_token,
                rx,
            )
            .await;
        });
        Ok(transfer_id)
    }

    async fn outgoing_task(
        self: Arc<Self>,
        peer: String,
        file: SendFile,
        file_name: String,
        transfer_id: String,
        size: u64,
        mtime: Option<i64>,
        resume_token: String,
        mut rx: mpsc::Receiver<OutgoingMsg>,
    ) {
        let rel_path = if file.rel_parts.is_empty() {
            None
        } else {
            Some(file.rel_parts.join("/"))
        };
        // 1) 提议
        let offer = Control::FileOffer {
            transfer_id: transfer_id.clone(),
            name: file_name.clone(),
            size,
            sha256: None, // 发送前不整文件预哈希（避免二次读盘），以 FileDone 哈希为准
            mtime_ms: mtime,
            rel_path,
            resume_token: Some(resume_token),
        };
        let mut failed: Option<String> = None;
        if let Err(e) = self.send_control(&peer, offer).await {
            failed = Some(format!("发送文件提议失败: {}", e));
        } else {
            tracing::debug!("[out {}] 提议已发送 name={}", transfer_id, file_name);
            self.emit_update(
                self.outgoing
                    .lock()
                    .unwrap()
                    .get(&transfer_id)
                    .unwrap()
                    .into_info(),
            );
        }

        // 2) 等待接受/拒绝/取消
        let mut offset = 0u64;
        let mut accepted = false;
        if failed.is_none() {
            let wait = tokio::time::timeout(ACCEPT_TIMEOUT, rx.recv());
            match wait.await {
                Ok(Some(OutgoingMsg::Control(Control::FileAccept {
                    offset: off,
                    partial_sha256,
                    ..
                }))) => {
                    if off > 0 {
                        // 校验已收部分哈希
                        let ok = match partial_sha256 {
                            Some(hash) => self.partial_matches(&file.path, off, &hash).await,
                            None => false,
                        };
                        if ok {
                            offset = off;
                        } else {
                            // 不一致：要求重头
                            let _ = self
                                .send_control(
                                    &peer,
                                    Control::FileRestart {
                                        transfer_id: transfer_id.clone(),
                                    },
                                )
                                .await;
                            offset = 0;
                        }
                    }
                    accepted = true;
                }
                Ok(Some(OutgoingMsg::Control(Control::FileReject { reason, .. }))) => {
                    failed = Some(format!("对端拒绝: {}", reason));
                    self.finish_outgoing(&transfer_id, TransferState::Rejected, failed.clone())
                        .await;
                }
                Ok(Some(OutgoingMsg::Control(Control::FileCancel { reason, .. }))) => {
                    failed = Some(format!("对端取消: {}", reason));
                    self.finish_outgoing(&transfer_id, TransferState::Canceled, failed.clone())
                        .await;
                }
                Ok(Some(OutgoingMsg::Disconnected(reason))) => {
                    failed = Some(format!("连接断开（可续传）: {}", reason));
                    self.finish_outgoing(&transfer_id, TransferState::Failed, failed.clone())
                        .await;
                }
                Ok(None) | Err(_) => {
                    failed = Some("等待确认超时".into());
                    self.finish_outgoing(&transfer_id, TransferState::Failed, failed.clone())
                        .await;
                }
                _ => {
                    failed = Some("收到意外的控制消息".into());
                    self.finish_outgoing(&transfer_id, TransferState::Failed, failed.clone())
                        .await;
                }
            }
        }

        // 3) 流式发送（对端拒绝/取消/断连/超时时条目已被清理，这里直接结束）
        let Some(mut state) = self.outgoing.lock().unwrap().get(&transfer_id).cloned() else {
            return;
        };
        state.state = TransferState::InProgress;
        state.resume_offset = offset;
        {
            let mut map = self.outgoing.lock().unwrap();
            let s = map.get_mut(&transfer_id).unwrap();
            s.state = TransferState::InProgress;
            s.resume_offset = offset;
            s.sent = offset;
        }
        if accepted && failed.is_none() {
            tracing::debug!(
                "[out {}] 已接受，开始流式发送 offset={}",
                transfer_id,
                offset
            );
            self.emit_update(
                self.outgoing
                    .lock()
                    .unwrap()
                    .get(&transfer_id)
                    .unwrap()
                    .into_info(),
            );
            let f = tokio::fs::File::open(&file.path).await;
            match f {
                Ok(mut fh) => {
                    let mut prefix_failed = false;
                    let mut hasher = Sha256::new();
                    if offset > 0 {
                        // FileDone 必须携带整文件哈希。续传时先顺序读完已确认前缀，
                        // 这样文件游标正好停在 offset，同时哈希覆盖完整内容。
                        let mut remaining = offset;
                        let mut prefix = vec![0u8; CHUNK_SIZE];
                        while remaining > 0 {
                            let want = remaining.min(prefix.len() as u64) as usize;
                            match fh.read(&mut prefix[..want]).await {
                                Ok(0) => {
                                    failed = Some("续传源文件短于已接收偏移".into());
                                    prefix_failed = true;
                                    break;
                                }
                                Ok(n) => {
                                    hasher.update(&prefix[..n]);
                                    remaining -= n as u64;
                                }
                                Err(e) => {
                                    failed = Some(format!("读取续传前缀失败: {}", e));
                                    prefix_failed = true;
                                    break;
                                }
                            }
                        }
                    }
                    let mut sent = offset;
                    let mut seq: u64 = 0;
                    let mut in_flight: u64 = 0;
                    let mut buf = vec![0u8; CHUNK_SIZE];
                    let mut eof = prefix_failed; // 前缀读取失败则不再发送任何分块
                    while !eof {
                        let is_paused = self
                            .outgoing_paused
                            .lock()
                            .unwrap()
                            .get(&transfer_id)
                            .copied()
                            .unwrap_or(false);
                        if is_paused {
                            if let Some(s) = self.outgoing.lock().unwrap().get_mut(&transfer_id) {
                                if s.state != TransferState::Paused {
                                    s.state = TransferState::Paused;
                                    self.emit_update(s.into_info());
                                }
                            }
                            tokio::time::sleep(Duration::from_millis(120)).await;
                            continue;
                        } else if let Some(s) = self.outgoing.lock().unwrap().get_mut(&transfer_id)
                        {
                            if s.state == TransferState::Paused {
                                s.state = TransferState::InProgress;
                                self.emit_update(s.into_info());
                            }
                        }
                        while in_flight < WINDOW_CHUNKS && !eof {
                            let n = match fh.read(&mut buf).await {
                                Ok(n) => n,
                                Err(e) => {
                                    failed = Some(format!("读取源文件失败: {}", e));
                                    eof = true;
                                    break;
                                }
                            };
                            if n == 0 {
                                eof = true;
                                break;
                            }
                            hasher.update(&buf[..n]);
                            sent += n as u64;
                            seq += 1;
                            let mut payload = Vec::with_capacity(CHUNK_HEADER_LEN + n);
                            payload.extend_from_slice(transfer_id.as_bytes());
                            payload.extend_from_slice(&(sent - n as u64).to_le_bytes());
                            payload.extend_from_slice(&seq.to_le_bytes());
                            payload.extend_from_slice(&buf[..n]);
                            if self.send_chunk(&peer, payload).await.is_err() {
                                failed = Some("发送分块失败（连接断开，可续传）".into());
                                eof = true;
                                break;
                            }
                            in_flight += 1;
                        }
                        if eof {
                            break;
                        }
                        // 等待 ACK
                        match tokio::time::timeout(ACK_TIMEOUT, rx.recv()).await {
                            Ok(Some(OutgoingMsg::Control(Control::ChunkAck {
                                seq: ack_seq,
                                offset: ack_offset,
                                ..
                            }))) => {
                                tracing::debug!(
                                    "出站 ack transfer={} seq={} offset={} in_flight={}",
                                    transfer_id,
                                    ack_seq,
                                    ack_offset,
                                    in_flight
                                );
                                in_flight = in_flight
                                    .saturating_sub(ack_seq.saturating_sub(state.chunk_seq));
                                state.chunk_seq = ack_seq;
                                state.sent = ack_offset;
                                self.throttled_emit_outgoing(&transfer_id, &mut state).await;
                            }
                            Ok(Some(OutgoingMsg::Control(Control::FileCancel {
                                reason, ..
                            }))) => {
                                // 本地或远端取消：通知对端（若来自本地 UI）
                                failed = Some(format!("对端取消: {}", reason));
                                let _ = self
                                    .send_control(
                                        &peer,
                                        Control::FileCancel {
                                            transfer_id: transfer_id.clone(),
                                            reason: reason.clone(),
                                        },
                                    )
                                    .await;
                                break;
                            }
                            Ok(Some(OutgoingMsg::Control(Control::FileDone { .. }))) => {
                                // 对端提前完成（异常）
                                failed = Some("对端提前完成".into());
                                break;
                            }
                            Ok(Some(OutgoingMsg::Disconnected(reason))) => {
                                failed = Some(format!("连接断开（可续传）: {}", reason));
                                break;
                            }
                            Ok(None) | Err(_) => {
                                tracing::warn!(
                                    "出站 ack 超时 transfer={} in_flight={}",
                                    transfer_id,
                                    in_flight
                                );
                                failed = Some("等待 ACK 超时（可续传）".into());
                                break;
                            }
                            _ => {
                                failed = Some("收到意外的控制消息".into());
                                break;
                            }
                        }
                    }
                    let final_hash = hex(&hasher.finalize());
                    if failed.is_none() {
                        // 4) 完成
                        let done = Control::FileDone {
                            transfer_id: transfer_id.clone(),
                            sha256: final_hash,
                            size: sent,
                        };
                        if self.send_control(&peer, done).await.is_ok() {
                            // 最后一个分块的 ACK 与完成回执都由接收端顺序发送。
                            // 发送循环到达 EOF 时可能尚未消费最终 ACK，因此完成阶段
                            // 必须继续接收合法 ACK，直到真正拿到 FileDone。
                            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
                            loop {
                                match tokio::time::timeout_at(deadline, rx.recv()).await {
                                    Ok(Some(OutgoingMsg::Control(Control::ChunkAck {
                                        seq: ack_seq,
                                        offset: ack_offset,
                                        ..
                                    }))) => {
                                        state.chunk_seq = ack_seq;
                                        state.sent = ack_offset;
                                        self.throttled_emit_outgoing(&transfer_id, &mut state)
                                            .await;
                                    }
                                    Ok(Some(OutgoingMsg::Control(Control::FileDone {
                                        ..
                                    }))) => {
                                        break;
                                    }
                                    Ok(Some(OutgoingMsg::Control(Control::FileCancel {
                                        reason,
                                        ..
                                    }))) => {
                                        failed = Some(format!("对端校验失败: {}", reason));
                                        break;
                                    }
                                    Ok(Some(OutgoingMsg::Disconnected(reason))) => {
                                        failed =
                                            Some(format!("连接断开（完成未确认）: {}", reason));
                                        break;
                                    }
                                    Ok(Some(_)) => continue,
                                    Ok(None) | Err(_) => {
                                        failed = Some("完成回执超时".into());
                                        break;
                                    }
                                }
                            }
                        } else {
                            failed = Some("发送完成帧失败".into());
                        }
                    }
                }
                Err(e) => {
                    failed = Some(format!("打开本地文件失败: {}", e));
                }
            }
        }

        if let Some(err) = failed {
            self.finish_outgoing(&transfer_id, TransferState::Failed, Some(err))
                .await;
        } else {
            self.finish_outgoing(&transfer_id, TransferState::Done, None)
                .await;
        }
        // 清理
        self.outgoing_tx.lock().unwrap().remove(&transfer_id);
        self.outgoing_paused.lock().unwrap().remove(&transfer_id);
    }

    async fn finish_outgoing(
        &self,
        transfer_id: &str,
        state: TransferState,
        error: Option<String>,
    ) {
        if let Some(st) = self.outgoing.lock().unwrap().get_mut(transfer_id) {
            st.state = state;
            st.error = error;
            let info = st.into_info();
            self.emit_update(info);
        }
        self.outgoing.lock().unwrap().remove(transfer_id);
    }

    async fn partial_matches(&self, path: &Path, offset: u64, expected: &str) -> bool {
        let Ok(mut f) = tokio::fs::File::open(path).await else {
            return false;
        };
        let mut hasher = Sha256::new();
        let mut remaining = offset;
        let mut buf = vec![0u8; 1024 * 1024];
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let Ok(n) = f.read(&mut buf[..want]).await else {
                return false;
            };
            if n == 0 {
                return false;
            }
            hasher.update(&buf[..n]);
            remaining -= n as u64;
        }
        hex(&hasher.finalize()) == expected
    }

    async fn forward_outgoing(&self, _peer: &str, control: Control) -> Result<()> {
        let id = transfer_id_of(&control).to_string();
        let tx = self
            .outgoing_tx
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow!("发送任务不存在: {}", id))?;
        tx.send(OutgoingMsg::Control(control))
            .await
            .map_err(|_| anyhow!("发送任务已结束"))?;
        Ok(())
    }

    // ---------- 事件与记录 ----------

    fn emit_update(&self, info: TransferInfo) {
        let _ = self.store.append_transfer(&info);
        self.bus.emit(CoreEvent::TransferUpdate(info));
    }

    async fn throttled_emit(&self, transfer_id: &str, _state: IncomingState, direction: Direction) {
        let now = Instant::now();
        let info = {
            let mut map = self.incoming.lock().unwrap();
            let Some(st) = map.get_mut(transfer_id) else {
                return;
            };
            if let Some(last) = st.last_emit {
                let elapsed = now.duration_since(last);
                if elapsed < Duration::from_millis(250) {
                    return;
                }
                let bytes = st.received.saturating_sub(st.last_emit_bytes);
                let sample = (bytes as f64 / elapsed.as_secs_f64()) as u64;
                st.speed_bps = smooth_speed(st.speed_bps, sample);
            }
            st.last_emit = Some(now);
            st.last_emit_bytes = st.received;
            let mut info = st.clone_for_send().into_info(None);
            info.direction = direction;
            info
        };
        self.emit_update(info);
    }

    async fn throttled_emit_outgoing(&self, transfer_id: &str, state: &mut OutgoingState) {
        let now = Instant::now();
        if let Some(last) = state.last_emit {
            let elapsed = now.duration_since(last);
            if elapsed < Duration::from_millis(250) {
                return;
            }
            let bytes = state.sent.saturating_sub(state.last_emit_bytes);
            let sample = (bytes as f64 / elapsed.as_secs_f64()) as u64;
            state.speed_bps = smooth_speed(state.speed_bps, sample);
        }
        state.last_emit = Some(now);
        state.last_emit_bytes = state.sent;
        let mut map = self.outgoing.lock().unwrap();
        if let Some(st) = map.get_mut(transfer_id) {
            st.sent = state.sent;
            st.chunk_seq = state.chunk_seq;
            st.last_emit = state.last_emit;
            st.last_emit_bytes = state.last_emit_bytes;
            st.speed_bps = state.speed_bps;
        }
        drop(map);
        self.emit_update(state.into_info());
    }
}

// ---------- 辅助 ----------

fn transfer_id_of(control: &Control) -> &str {
    match control {
        Control::FileAccept { transfer_id, .. }
        | Control::FileReject { transfer_id, .. }
        | Control::FileRestart { transfer_id }
        | Control::ChunkAck { transfer_id, .. }
        | Control::FileDone { transfer_id, .. }
        | Control::FileCancel { transfer_id, .. } => transfer_id,
        _ => "",
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn smooth_speed(previous: u64, sample: u64) -> u64 {
    if previous == 0 {
        sample
    } else {
        ((previous as u128 * 7 + sample as u128 * 3) / 10) as u64
    }
}

fn hex(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 传输 ID 必须是 36 字符 UUID（8-4-4-4-12，十六进制 + 连字符）。
/// 它会被拼接进临时文件路径，必须白名单校验，防目录穿越。
fn is_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        match i {
            8 | 13 | 18 | 23 => {
                if *b != b'-' {
                    return false;
                }
            }
            _ => {
                if !b.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    true
}

/// 目标路径去重：已存在则追加 " (1)"、" (2)"…
fn unique_path(dir: &Path, rel_parts: &[String], name: &str) -> PathBuf {
    let mut base = dir.to_path_buf();
    for p in rel_parts {
        base = base.join(p);
    }
    let candidate = base.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    let ext = Path::new(name)
        .extension()
        .map(|s| s.to_string_lossy().to_string());
    let mut i = 1;
    loop {
        let new_name = match &ext {
            Some(e) => format!("{} ({}).{}", stem, i, e),
            None => format!("{} ({})", stem, i),
        };
        let candidate = base.join(&new_name);
        if !candidate.exists() {
            return candidate;
        }
        i += 1;
    }
}

impl IncomingState {
    fn clone_for_send(&self) -> IncomingState {
        IncomingState {
            transfer_id: self.transfer_id.clone(),
            peer: self.peer.clone(),
            file_name: self.file_name.clone(),
            rel_parts: self.rel_parts.clone(),
            size: self.size,
            sha256_expected: self.sha256_expected.clone(),
            resume_token: self.resume_token.clone(),
            state: self.state,
            dest_dir: self.dest_dir.clone(),
            temp_path: self.temp_path.clone(),
            local_path: self.local_path.clone(),
            file: None,
            hasher: None,
            received: self.received,
            partial_hash: self.partial_hash.clone(),
            error: self.error.clone(),
            chunk_seq: self.chunk_seq,
            last_emit: self.last_emit,
            last_emit_bytes: self.last_emit_bytes,
            speed_bps: self.speed_bps,
            ts_ms: self.ts_ms,
        }
    }

    fn into_info(self, error: Option<String>) -> TransferInfo {
        TransferInfo {
            transfer_id: self.transfer_id,
            peer_device_id: self.peer,
            direction: Direction::Incoming,
            state: self.state,
            file_name: self.file_name,
            file_size: self.size,
            transferred: self.received,
            speed_bps: self.speed_bps,
            error,
            resume_offset: self.received,
            local_path: self
                .local_path
                .map(|path| path.to_string_lossy().to_string()),
            ts_ms: self.ts_ms,
        }
    }
}

impl OutgoingState {
    fn into_info(&self) -> TransferInfo {
        TransferInfo {
            transfer_id: self.transfer_id.clone(),
            peer_device_id: self.peer.clone(),
            direction: Direction::Outgoing,
            state: self.state,
            file_name: self.file_name.clone(),
            file_size: self.size,
            transferred: self.sent,
            speed_bps: self.speed_bps,
            error: self.error.clone(),
            resume_offset: self.resume_offset,
            local_path: Some(self.path.to_string_lossy().to_string()),
            ts_ms: self.ts_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_id_must_be_uuid() {
        // 正常 UUID（send 端生成）通过
        assert!(is_uuid("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_uuid("3b476564-2ec9-4241-a441-cfe921a60c96"));
        // 目录穿越/非法字符一律拒绝
        assert!(!is_uuid("../evil"));
        assert!(!is_uuid("..\\evil"));
        assert!(!is_uuid("550e8400e29b41d4a716446655440000")); // 无连字符
        assert!(!is_uuid("550e8400-e29b-41d4-a716-44665544000g")); // 非 hex
        assert!(!is_uuid(""));
        assert!(!is_uuid("550e8400-e29b-41d4-a716-446655440000/../../x"));
        assert!(!is_uuid("a".repeat(36).as_str())); // 全 a 但无连字符位置
    }

    #[test]
    fn speed_samples_are_smoothed() {
        assert_eq!(smooth_speed(0, 1_000), 1_000);
        assert_eq!(smooth_speed(1_000, 2_000), 1_300);
    }
}
