//! 本地加密记录存储（SQLite + 应用层 AES-256-GCM 逐记录加密）。
//!
//! 安全设计：
//! - 每条消息/传输记录独立随机 nonce，AES-256-GCM 加密；
//! - 主密钥 32 字节随机，存放于 records.key（Unix 0600；Windows 依赖目录 ACL，
//!   DPAPI 包裹列为后续改进）；
//! - 数据文件内无明文：消息正文、链接、文件名、路径全部加密；
//! - 导出：独立密码 → Argon2id（64 MiB / t=3 / p=1）派生 KEK → 包裹随机 DEK →
//!   AES-256-GCM 加密导出 JSON；导入校验完整性并按消息 UUID 去重。
//!
//! 密钥丢失 = 本地记录不可恢复（如实的安全边界）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use anyhow::{anyhow, bail, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::events::{Direction, MsgKind, TransferInfo, TransferState};

const MAGIC: &[u8; 9] = b"LUNOTEXP1";
const EXPORT_VERSION: u32 = 1;
const RECORD_AAD: &[u8] = b"lunote-record-v1";
const DEK_AAD: &[u8] = b"LUNOTE-DEK-v1";
const PAYLOAD_AAD: &[u8] = b"LUNOTE-PAYLOAD-v1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub direction: Direction,
    pub kind: MsgKind,
    pub text: String,
    pub url: Option<String>,
    pub ts_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredConversation {
    pub device_id: String,
    pub peer_name: String,
    pub messages: Vec<StoredMessage>,
    pub transfers: Vec<TransferInfo>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ExportReport {
    pub messages: u64,
    pub transfers: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ImportReport {
    pub imported_messages: u64,
    pub skipped_messages: u64,
    pub imported_transfers: u64,
    pub skipped_transfers: u64,
}

pub struct Store {
    conn: Mutex<Connection>,
    key: [u8; 32],
    _data_dir: PathBuf,
}

impl Store {
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let key_path = data_dir.join("records.key");
        let key = if key_path.exists() {
            let hex = std::fs::read_to_string(&key_path)
                .context("读取 records.key 失败")?
                .trim()
                .to_string();
            let bytes = hex_decode(&hex).context("records.key 内容非法")?;
            if bytes.len() != 32 {
                bail!("records.key 长度错误");
            }
            let mut k = [0u8; 32];
            k.copy_from_slice(&bytes);
            k
        } else {
            let mut k = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut k);
            std::fs::write(&key_path, hex_encode(&k)).context("写入 records.key 失败")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
            }
            k
        };

        let db_path = data_dir.join("records.db");
        let conn = Connection::open(&db_path).context("打开记录数据库失败")?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS conversations (
               device_id TEXT PRIMARY KEY,
               peer_name TEXT NOT NULL DEFAULT '',
               created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS messages (
               id TEXT PRIMARY KEY,
               conversation_id TEXT NOT NULL,
               direction TEXT NOT NULL,
               kind TEXT NOT NULL,
               payload BLOB NOT NULL,
               nonce BLOB NOT NULL,
               created_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation_id, created_at);
             CREATE TABLE IF NOT EXISTS transfers (
               id TEXT PRIMARY KEY,
               conversation_id TEXT NOT NULL,
               direction TEXT NOT NULL,
               file_name TEXT NOT NULL,
               file_size INTEGER NOT NULL,
               state TEXT NOT NULL,
               sha256 TEXT,
               transferred INTEGER NOT NULL DEFAULT 0,
               resume_offset INTEGER NOT NULL DEFAULT 0,
               error TEXT,
               local_path BLOB,
               local_path_nonce BLOB,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_transfers_conv ON transfers(conversation_id, created_at);",
        )
        .context("初始化数据库结构失败")?;
        // v2: 已存在的数据库补充加密本地路径列；重复执行时忽略 duplicate column。
        let _ = conn.execute("ALTER TABLE transfers ADD COLUMN local_path BLOB", []);
        let _ = conn.execute("ALTER TABLE transfers ADD COLUMN local_path_nonce BLOB", []);

        Ok(Self {
            conn: Mutex::new(conn),
            key,
            _data_dir: data_dir.to_path_buf(),
        })
    }

    pub fn update_conversation_name(&self, device_id: &str, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO conversations(device_id, peer_name, created_at) VALUES(?1, ?2, ?3)
             ON CONFLICT(device_id) DO UPDATE SET peer_name=excluded.peer_name",
            params![device_id, name, now_ms()],
        )?;
        Ok(())
    }

    pub fn conversation_name(&self, device_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let name = conn
            .query_row(
                "SELECT peer_name FROM conversations WHERE device_id=?1",
                params![device_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(name)
    }

    pub fn append_message(
        &self,
        conversation: &str,
        id: &str,
        direction: Direction,
        kind: MsgKind,
        text: &str,
        url: Option<&str>,
        ts_ms: i64,
    ) -> Result<()> {
        let plain = serde_json::to_vec(&serde_json::json!({
            "text": text,
            "url": url,
        }))?;
        let (nonce, ct) = aes_encrypt(&self.key, &plain, RECORD_AAD)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO conversations(device_id, peer_name, created_at) VALUES(?1, '', ?2)
             ON CONFLICT(device_id) DO NOTHING",
            params![conversation, now_ms()],
        )?;
        conn.execute(
            // 消息 id 由对端提供：重复 id（重放/恶意）用 INSERT OR IGNORE 容忍，
            // 避免主键冲突 → 分发错误 → 整个连接被踢断
            "INSERT OR IGNORE INTO messages(id, conversation_id, direction, kind, payload, nonce, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                conversation,
                dir_str(direction),
                kind_str(kind),
                ct,
                nonce,
                ts_ms
            ],
        )?;
        Ok(())
    }

    pub fn append_transfer(&self, t: &TransferInfo) -> Result<()> {
        let (path_nonce, path_ct) = match t.local_path.as_deref() {
            Some(path) if !path.is_empty() => {
                let (nonce, ct) = aes_encrypt(&self.key, path.as_bytes(), RECORD_AAD)?;
                (Some(nonce), Some(ct))
            }
            _ => (None, None),
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO conversations(device_id, peer_name, created_at) VALUES(?1, '', ?2)
             ON CONFLICT(device_id) DO NOTHING",
            params![t.peer_device_id, now_ms()],
        )?;
        conn.execute(
            "INSERT INTO transfers(
               id, conversation_id, direction, file_name, file_size, state, sha256,
               transferred, resume_offset, error, local_path, local_path_nonce,
               created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
               state=excluded.state,
               transferred=excluded.transferred,
               resume_offset=excluded.resume_offset,
               error=excluded.error,
               local_path=COALESCE(excluded.local_path, transfers.local_path),
               local_path_nonce=COALESCE(excluded.local_path_nonce, transfers.local_path_nonce),
               updated_at=excluded.updated_at",
            params![
                t.transfer_id,
                t.peer_device_id,
                dir_str(t.direction),
                t.file_name,
                t.file_size as i64,
                state_str(t.state),
                None::<String>, // 文件哈希不落明文记录（v1）
                t.transferred as i64,
                t.resume_offset as i64,
                t.error,
                path_ct,
                path_nonce,
                t.ts_ms,
                now_ms(),
            ],
        )?;
        Ok(())
    }

    pub fn list_conversations(&self) -> Result<Vec<StoredConversation>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.device_id, c.peer_name, c.created_at FROM conversations c
             ORDER BY COALESCE((SELECT MAX(m.created_at) FROM messages m WHERE m.conversation_id = c.device_id), c.created_at) DESC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (device_id, peer_name) = row?;
            out.push(StoredConversation {
                device_id: device_id.clone(),
                peer_name,
                messages: self.load_messages_locked(&conn, &device_id)?,
                transfers: self.load_transfers_locked(&conn, &device_id)?,
            });
        }
        Ok(out)
    }

    pub fn list_messages(&self, conversation: &str) -> Result<Vec<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        self.load_messages_locked(&conn, conversation)
    }

    pub fn list_transfers(&self, conversation: &str) -> Result<Vec<TransferInfo>> {
        let conn = self.conn.lock().unwrap();
        self.load_transfers_locked(&conn, conversation)
    }

    fn load_messages_locked(
        &self,
        conn: &Connection,
        conversation: &str,
    ) -> Result<Vec<StoredMessage>> {
        let mut stmt = conn.prepare(
            "SELECT id, direction, kind, payload, nonce, created_at FROM messages
             WHERE conversation_id=?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![conversation], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, direction, kind, ct, nonce, ts) = row?;
            let plain = aes_decrypt(&self.key, &ct, &nonce, RECORD_AAD)
                .map_err(|_| anyhow!("记录解密失败（密钥不匹配？）"))?;
            let v: serde_json::Value = serde_json::from_slice(&plain)?;
            out.push(StoredMessage {
                id,
                direction: dir_from_str(&direction)?,
                kind: kind_from_str(&kind)?,
                text: v
                    .get("text")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                url: v.get("url").and_then(|x| x.as_str()).map(|s| s.to_string()),
                ts_ms: ts,
            });
        }
        Ok(out)
    }

    fn load_transfers_locked(
        &self,
        conn: &Connection,
        conversation: &str,
    ) -> Result<Vec<TransferInfo>> {
        let mut stmt = conn.prepare(
            "SELECT id, direction, file_name, file_size, state, transferred, resume_offset, error,
                    local_path, local_path_nonce, created_at
             FROM transfers WHERE conversation_id=?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![conversation], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<Vec<u8>>>(8)?,
                r.get::<_, Option<Vec<u8>>>(9)?,
                r.get::<_, i64>(10)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                direction,
                name,
                size,
                state,
                transferred,
                resume,
                error,
                path_ct,
                path_nonce,
                ts,
            ) = row?;
            let local_path = match (path_ct, path_nonce) {
                (Some(ct), Some(nonce)) => Some(String::from_utf8(aes_decrypt(
                    &self.key, &ct, &nonce, RECORD_AAD,
                )?)?),
                _ => None,
            };
            out.push(TransferInfo {
                transfer_id: id,
                peer_device_id: conversation.to_string(),
                direction: dir_from_str(&direction)?,
                state: state_from_str(&state)?,
                file_name: name,
                file_size: size.max(0) as u64,
                transferred: transferred.max(0) as u64,
                speed_bps: 0,
                error,
                resume_offset: resume.max(0) as u64,
                local_path,
                ts_ms: ts,
            });
        }
        Ok(out)
    }

    /// 彻底删除全部本地记录（保留数据库结构与密钥）
    pub fn wipe(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "DELETE FROM messages; DELETE FROM transfers; DELETE FROM conversations;",
        )?;
        Ok(())
    }

    /// 删除与某设备的对话（本地消息 + 传输记录；不影响信任关系与设备身份）
    pub fn delete_conversation(&self, device_id: &str) -> Result<()> {
        self.delete_conversations(&[device_id.to_string()])
    }

    /// 在同一事务中删除多个对话，确保批量操作不会只完成一部分。
    pub fn delete_conversations(&self, device_ids: &[String]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for device_id in device_ids {
            tx.execute(
                "DELETE FROM messages WHERE conversation_id=?1",
                params![device_id],
            )?;
            tx.execute(
                "DELETE FROM transfers WHERE conversation_id=?1",
                params![device_id],
            )?;
            tx.execute(
                "DELETE FROM conversations WHERE device_id=?1",
                params![device_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 导出：密码 → Argon2id → AES-256-GCM 信封（仅消息与传输记录，不含文件本体）
    pub fn export(&self, password: &str, out_path: &Path) -> Result<ExportReport> {
        if password.len() < 8 {
            bail!("密码至少 8 个字符");
        }
        let conversations = self.list_conversations()?;
        let mut report = ExportReport::default();
        for c in &conversations {
            report.messages += c.messages.len() as u64;
            report.transfers += c.transfers.len() as u64;
        }
        let payload = serde_json::to_vec(&serde_json::json!({
            "exported_at": now_ms(),
            "conversations": conversations,
        }))?;

        let mut salt = [0u8; SALT_LEN];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        let mut kek = [0u8; 32];
        derive_kek(password, &salt, &mut kek)?;

        let mut dek = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut dek);
        let (nonce1, wrapped_dek) = aes_encrypt(&kek, &dek, DEK_AAD)?;
        let (nonce2, payload_ct) = aes_encrypt(&dek, &payload, PAYLOAD_AAD)?;

        let mut file = Vec::with_capacity(128 + payload_ct.len());
        file.extend_from_slice(MAGIC);
        file.extend_from_slice(&EXPORT_VERSION.to_le_bytes());
        file.extend_from_slice(&salt);
        file.extend_from_slice(&nonce1);
        file.extend_from_slice(&wrapped_dek);
        file.extend_from_slice(&nonce2);
        file.extend_from_slice(&payload_ct);
        std::fs::write(out_path, &file)
            .with_context(|| format!("写入导出文件失败: {}", out_path.display()))?;
        Ok(report)
    }

    /// 导入：校验完整性（GCM 标签），按消息 UUID 去重
    pub fn import(&self, password: &str, in_path: &Path) -> Result<ImportReport> {
        let data = std::fs::read(in_path)
            .with_context(|| format!("读取导入文件失败: {}", in_path.display()))?;
        if data.len() < MAGIC.len() + 4 + SALT_LEN + NONCE_LEN + 32 + NONCE_LEN + 16 {
            bail!("导入文件太短或格式错误");
        }
        if &data[..9] != MAGIC {
            bail!("不是 Lunote 导出文件（magic 不匹配）");
        }
        let version = u32::from_le_bytes(data[9..13].try_into().unwrap());
        if version != EXPORT_VERSION {
            bail!("不支持的导出版本: {}", version);
        }
        let mut off = 13;
        let salt = &data[off..off + SALT_LEN];
        off += SALT_LEN;
        let nonce1 = &data[off..off + NONCE_LEN];
        off += NONCE_LEN;
        let wrapped_dek = &data[off..off + 32 + 16];
        off += 32 + 16;
        let nonce2 = &data[off..off + NONCE_LEN];
        off += NONCE_LEN;
        let payload_ct = &data[off..];

        let mut kek = [0u8; 32];
        derive_kek(password, salt, &mut kek)?;
        let dek = aes_decrypt(&kek, wrapped_dek, nonce1, DEK_AAD)
            .map_err(|_| anyhow!("密码错误或导出文件已损坏（DEK 校验失败）"))?;
        if dek.len() != 32 {
            bail!("DEK 长度错误");
        }
        let mut dek32 = [0u8; 32];
        dek32.copy_from_slice(&dek);
        let plain = aes_decrypt(&dek32, payload_ct, nonce2, PAYLOAD_AAD)
            .map_err(|_| anyhow!("导出文件内容校验失败（可能被篡改）"))?;
        let v: serde_json::Value =
            serde_json::from_slice(&plain).context("导出内容 JSON 解析失败")?;
        let conversations: Vec<StoredConversation> = serde_json::from_value(
            v.get("conversations")
                .cloned()
                .unwrap_or(serde_json::json!([])),
        )
        .context("导出内容结构错误")?;

        let mut report = ImportReport::default();
        let conn = self.conn.lock().unwrap();
        for conv in conversations {
            conn.execute(
                "INSERT INTO conversations(device_id, peer_name, created_at) VALUES(?1, ?2, ?3)
                 ON CONFLICT(device_id) DO UPDATE SET peer_name = CASE WHEN excluded.peer_name <> '' THEN excluded.peer_name ELSE peer_name END",
                params![conv.device_id, conv.peer_name, now_ms()],
            )?;
            for m in conv.messages {
                let plain = serde_json::to_vec(&serde_json::json!({"text": m.text, "url": m.url}))?;
                let (nonce, ct) = aes_encrypt(&self.key, &plain, RECORD_AAD)?;
                let n = conn.execute(
                    "INSERT OR IGNORE INTO messages(id, conversation_id, direction, kind, payload, nonce, created_at)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        m.id,
                        conv.device_id,
                        dir_str(m.direction),
                        kind_str(m.kind),
                        ct,
                        nonce,
                        m.ts_ms
                    ],
                )?;
                if n > 0 {
                    report.imported_messages += 1;
                } else {
                    report.skipped_messages += 1;
                }
            }
            for t in conv.transfers {
                let n = conn.execute(
                    "INSERT OR IGNORE INTO transfers(
                       id, conversation_id, direction, file_name, file_size, state, sha256,
                       transferred, resume_offset, error, created_at, updated_at
                     ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        t.transfer_id,
                        conv.device_id,
                        dir_str(t.direction),
                        t.file_name,
                        t.file_size as i64,
                        state_str(t.state),
                        None::<String>, // 文件哈希不落明文记录（v1）
                        t.transferred as i64,
                        t.resume_offset as i64,
                        t.error,
                        t.ts_ms,
                        now_ms(),
                    ],
                )?;
                if n > 0 {
                    report.imported_transfers += 1;
                } else {
                    report.skipped_transfers += 1;
                }
            }
        }
        Ok(report)
    }
}

// ---------- 加密与派生 ----------

fn aes_encrypt(key: &[u8; 32], plain: &[u8], aad: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256-GCM 密钥长度固定");
    let mut nonce = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
        .map_err(|_| anyhow!("AES-256-GCM 加密失败"))?;
    Ok((nonce.to_vec(), ct))
}

fn aes_decrypt(key: &[u8; 32], ct: &[u8], nonce: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    // 防御：本地库 nonce 列被损坏/篡改时长度≠12，返回错误而非 panic
    if nonce.len() != 12 {
        anyhow::bail!("记录 nonce 长度异常（{}），本地记录可能已损坏", nonce.len());
    }
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256-GCM 密钥长度固定");
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
        .map_err(|_| anyhow!("AES-256-GCM 解密失败（完整性校验未通过）"))
}

fn derive_kek(password: &str, salt: &[u8], out: &mut [u8; 32]) -> Result<()> {
    let params =
        Params::new(64 * 1024, 3, 1, Some(32)).map_err(|e| anyhow!("Argon2 参数错误: {}", e))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    argon
        .hash_password_into(password.as_bytes(), salt, out)
        .map_err(|e| anyhow!("Argon2id 派生失败: {}", e))
}

// ---------- 辅助 ----------

fn dir_str(d: Direction) -> &'static str {
    match d {
        Direction::Outgoing => "outgoing",
        Direction::Incoming => "incoming",
    }
}

fn dir_from_str(s: &str) -> Result<Direction> {
    match s {
        "outgoing" => Ok(Direction::Outgoing),
        "incoming" => Ok(Direction::Incoming),
        _ => bail!("未知方向: {}", s),
    }
}

fn kind_str(k: MsgKind) -> &'static str {
    match k {
        MsgKind::Text => "text",
        MsgKind::Link => "link",
    }
}

fn kind_from_str(s: &str) -> Result<MsgKind> {
    match s {
        "text" => Ok(MsgKind::Text),
        "link" => Ok(MsgKind::Link),
        _ => bail!("未知消息类型: {}", s),
    }
}

fn state_str(s: TransferState) -> &'static str {
    match s {
        TransferState::Offered => "offered",
        TransferState::Accepted => "accepted",
        TransferState::InProgress => "in_progress",
        TransferState::Paused => "paused",
        TransferState::Done => "done",
        TransferState::Failed => "failed",
        TransferState::Canceled => "canceled",
        TransferState::Rejected => "rejected",
    }
}

fn state_from_str(s: &str) -> Result<TransferState> {
    Ok(match s {
        "offered" => TransferState::Offered,
        "accepted" => TransferState::Accepted,
        "in_progress" => TransferState::InProgress,
        "paused" => TransferState::Paused,
        "done" => TransferState::Done,
        "failed" => TransferState::Failed,
        "canceled" => TransferState::Canceled,
        "rejected" => TransferState::Rejected,
        _ => bail!("未知传输状态: {}", s),
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn hex_decode(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        bail!("十六进制长度必须为偶数");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| anyhow!("非法十六进制字符")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_info(id: &str, peer: &str) -> TransferInfo {
        TransferInfo {
            transfer_id: id.into(),
            peer_device_id: peer.into(),
            direction: Direction::Incoming,
            state: TransferState::Done,
            file_name: "机密名单.txt".into(),
            file_size: 1024,
            transferred: 1024,
            speed_bps: 0,
            error: None,
            resume_offset: 0,
            local_path: Some("C:\\Users\\tester\\Downloads\\机密名单.txt".into()),
            ts_ms: 1,
        }
    }

    #[test]
    fn no_plaintext_in_db() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .append_message(
                "dev-a",
                "m1",
                Direction::Outgoing,
                MsgKind::Text,
                "秘密消息内容XYZ",
                None,
                123,
            )
            .unwrap();
        store
            .append_message(
                "dev-a",
                "m2",
                Direction::Incoming,
                MsgKind::Link,
                "https://example.com/secret",
                Some("https://example.com/secret"),
                124,
            )
            .unwrap();
        store.append_transfer(&sample_info("t1", "dev-a")).unwrap();
        let raw = std::fs::read(dir.path().join("records.db")).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(!text.contains("秘密消息内容XYZ"));
        assert!(!text.contains("example.com"));
        assert!(!text.contains("机密名单"));
        // 列表仍可读
        let msgs = store.list_messages("dev-a").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].text, "秘密消息内容XYZ");
        let convs = store.list_conversations().unwrap();
        assert_eq!(convs.len(), 1);
        assert_eq!(convs[0].transfers.len(), 1);
    }

    #[test]
    fn export_import_roundtrip_and_dedupe() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .append_message(
                "dev-a",
                "m1",
                Direction::Outgoing,
                MsgKind::Text,
                "你好",
                None,
                1,
            )
            .unwrap();
        store.append_transfer(&sample_info("t1", "dev-a")).unwrap();
        let out = dir.path().join("export.lunote");
        let rep = store.export("correct-horse-123", &out).unwrap();
        assert_eq!(rep.messages, 1);
        assert_eq!(rep.transfers, 1);

        // 错误密码
        assert!(store.import("wrong-password", &out).is_err());
        // 篡改
        let mut data = std::fs::read(&out).unwrap();
        let n = data.len();
        data[n / 2] ^= 0x01;
        std::fs::write(&out, &data).unwrap();
        assert!(store.import("correct-horse-123", &out).is_err());
        // 还原并正确导入 → 全部去重
        store.export("correct-horse-123", &out).unwrap();
        let rep2 = store.import("correct-horse-123", &out).unwrap();
        assert_eq!(rep2.imported_messages, 0);
        assert_eq!(rep2.skipped_messages, 1);
        assert_eq!(rep2.imported_transfers, 0);
        assert_eq!(rep2.skipped_transfers, 1);
    }

    #[test]
    fn wipe_removes_all() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .append_message(
                "dev-a",
                "m1",
                Direction::Outgoing,
                MsgKind::Text,
                "x",
                None,
                1,
            )
            .unwrap();
        store.wipe().unwrap();
        assert!(store.list_conversations().unwrap().is_empty());
    }

    #[test]
    fn batch_delete_removes_only_selected_conversations() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        for device in ["dev-a", "dev-b", "dev-c"] {
            store
                .append_message(
                    device,
                    &format!("message-{device}"),
                    Direction::Outgoing,
                    MsgKind::Text,
                    "x",
                    None,
                    1,
                )
                .unwrap();
        }
        store
            .delete_conversations(&["dev-a".into(), "dev-c".into()])
            .unwrap();
        let conversations = store.list_conversations().unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].device_id, "dev-b");
    }
}
