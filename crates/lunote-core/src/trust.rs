//! 信任与 TOFU 记录管理。
//!
//! - `trusted`：用户确认过的可信设备（可收发文件）。
//! - TOFU：所有见过指纹的设备都会被记录；同一 device_id 出现不同指纹 =
//!   身份变化警告，且该设备降级为不可信。
//!
//! 持久化为 JSON 文件（同目录临时文件 + 原子替换）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TrustRecord {
    pub device_id: String,
    pub name: String,
    pub fingerprint: String,
    pub trusted: bool,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    /// 最近一次在线的 IP（供“同名同 IP 自动信任”判定；旧数据兼容缺失）
    #[serde(default)]
    pub last_ip: Option<String>,
}

/// 身份核对结果
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityCheck {
    /// 全新设备（首次见面，TOFU 记录）
    New,
    /// 与记录一致（正常）
    Same,
    /// 身份指纹变化（潜在冒充/重装），必须警告并降级
    Changed,
}

#[derive(Default)]
pub struct TrustStore {
    path: Option<PathBuf>,
    records: HashMap<String, TrustRecord>,
}

impl TrustStore {
    pub fn load_or_create(path: &Path) -> Result<Self> {
        let mut store = Self {
            path: Some(path.to_path_buf()),
            records: HashMap::new(),
        };
        if path.exists() {
            let data = std::fs::read_to_string(path).context("读取信任库失败")?;
            let records: Vec<TrustRecord> =
                serde_json::from_str(&data).context("信任库 JSON 解析失败")?;
            for r in records {
                store.records.insert(r.device_id.clone(), r);
            }
        }
        Ok(store)
    }

    pub fn check_identity(
        &mut self,
        device_id: &str,
        fingerprint: &str,
        name: &str,
    ) -> IdentityCheck {
        let now = now_ms();
        let result = if let Some(rec) = self.records.get_mut(device_id) {
            rec.last_seen_ms = now;
            if !rec.name.is_empty() && rec.name != name && rec.trusted {
                rec.name = name.to_string();
            }
            if rec.fingerprint.is_empty() {
                // 预信任占位记录（设备页直接信任、尚未连接过的设备）：
                // 首次真实握手时填写实际指纹，保持信任不降级
                rec.fingerprint = fingerprint.to_string();
                IdentityCheck::Same
            } else if rec.fingerprint == fingerprint {
                IdentityCheck::Same
            } else {
                IdentityCheck::Changed
            }
        } else {
            self.records.insert(
                device_id.to_string(),
                TrustRecord {
                    device_id: device_id.to_string(),
                    name: name.to_string(),
                    fingerprint: fingerprint.to_string(),
                    trusted: false,
                    first_seen_ms: now,
                    last_seen_ms: now,
                    last_ip: None,
                },
            );
            IdentityCheck::New
        };
        let _ = self.flush();
        result
    }

    /// 用户确认信任（或解除信任）
    pub fn set_trusted(&mut self, device_id: &str, trusted: bool) -> Result<()> {
        let rec = self
            .records
            .get_mut(device_id)
            .context("该设备没有 TOFU 记录，无法设置信任")?;
        rec.trusted = trusted;
        self.flush()
    }

    /// 信任设备：有记录则更新；无记录则创建占位记录（指纹待首次连接确定）。
    /// 设备页“只发现未连接”的设备也能直接信任。
    pub fn trust_device(&mut self, device_id: &str, name: &str, trusted: bool) -> Result<()> {
        let now = now_ms();
        if let Some(rec) = self.records.get_mut(device_id) {
            rec.trusted = trusted;
            if !rec.name.is_empty() && rec.name != name {
                rec.name = name.to_string();
            }
        } else {
            self.records.insert(
                device_id.to_string(),
                TrustRecord {
                    device_id: device_id.to_string(),
                    name: name.to_string(),
                    fingerprint: String::new(), // 占位：首次连接时填写实际指纹
                    trusted,
                    first_seen_ms: now,
                    last_seen_ms: now,
                    last_ip: None,
                },
            );
        }
        self.flush()
    }

    pub fn is_trusted(&self, device_id: &str) -> bool {
        self.records
            .get(device_id)
            .map(|r| r.trusted)
            .unwrap_or(false)
    }

    pub fn get(&self, device_id: &str) -> Option<&TrustRecord> {
        self.records.get(device_id)
    }

    /// 删除设备记录（本地数据中的对话记录保留，信任关系清除）
    pub fn remove(&mut self, device_id: &str) -> Result<()> {
        self.records.remove(device_id);
        self.flush()
    }

    pub fn list(&self) -> Vec<TrustRecord> {
        let mut v: Vec<TrustRecord> = self.records.values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// 记录设备最近 IP（仅当记录存在；供“同名同 IP 自动信任”判定）
    pub fn update_last_ip(&mut self, device_id: &str, ip: &str) {
        if let Some(rec) = self.records.get_mut(device_id) {
            if rec.last_ip.as_deref() != Some(ip) {
                rec.last_ip = Some(ip.to_string());
                let _ = self.flush();
            }
        }
    }

    /// 是否存在“已信任且同名同 IP”的记录（新设备自动信任判定）
    pub fn auto_trust_match(&self, name: &str, ip: &str) -> bool {
        self.records
            .values()
            .any(|r| r.trusted && r.name == name && r.last_ip.as_deref() == Some(ip))
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    fn flush(&mut self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut v: Vec<&TrustRecord> = self.records.values().collect();
        v.sort_by(|a, b| a.device_id.cmp(&b.device_id));
        let data = serde_json::to_string_pretty(&v)?;
        crate::platform::atomic_write(path, data.as_bytes(), true).context("信任库原子写入失败")
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tofu_new_same_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.json");
        let mut s = TrustStore::load_or_create(&path).unwrap();
        assert_eq!(
            s.check_identity("dev-1", "fp-aaa", "甲"),
            IdentityCheck::New
        );
        assert_eq!(
            s.check_identity("dev-1", "fp-aaa", "甲"),
            IdentityCheck::Same
        );
        assert_eq!(
            s.check_identity("dev-1", "fp-bbb", "甲"),
            IdentityCheck::Changed
        );
        assert!(!s.is_trusted("dev-1"));
    }

    #[test]
    fn trust_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.json");
        {
            let mut s = TrustStore::load_or_create(&path).unwrap();
            s.check_identity("dev-2", "fp-2", "乙");
            s.set_trusted("dev-2", true).unwrap();
        }
        let s = TrustStore::load_or_create(&path).unwrap();
        assert!(s.is_trusted("dev-2"));
        assert_eq!(s.list().len(), 1);
    }

    #[test]
    fn repeated_trust_updates_replace_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.json");
        let mut store = TrustStore::load_or_create(&path).unwrap();
        store.check_identity("dev-3", "fp-3", "丙");
        store.set_trusted("dev-3", true).unwrap();
        store.set_trusted("dev-3", false).unwrap();
        let reloaded = TrustStore::load_or_create(&path).unwrap();
        assert!(!reloaded.is_trusted("dev-3"));
    }
}
