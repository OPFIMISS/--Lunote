//! Encrypted local notes with explicitly selected peers and vector-clock conflict copies.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::store::{aes_decrypt, aes_encrypt, derive_kek, Store};

const STORAGE_KEY: &str = "notes-workspace-v1";
const MAX_NOTES: usize = 512;
pub type Clock = BTreeMap<String, u64>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedBody {
    pub context: String,
    pub salt: Vec<u8>,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

impl LockedBody {
    fn seal(context: &str, plain: &[u8], password: &str) -> Result<Self> {
        if password.chars().count() < 8 || password.len() > 1024 {
            bail!("密码须为8到1024个字符");
        }
        let mut salt = vec![0; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        let mut key = [0; 32];
        derive_kek(password, &salt, &mut key)?;
        let encrypted = aes_encrypt(&key, plain, format!("lunote-note:{}", context).as_bytes());
        key.fill(0);
        let (nonce, ciphertext) = encrypted?;
        Ok(Self {
            context: context.into(),
            salt,
            nonce,
            ciphertext,
        })
    }

    fn open(&self, password: &str) -> Result<Vec<u8>> {
        if self.salt.len() != 16
            || self.nonce.len() != 12
            || self.context.len() > 128
            || password.len() > 1024
        {
            bail!("笔记密文格式非法");
        }
        let mut key = [0; 32];
        derive_kek(password, &self.salt, &mut key)?;
        let decrypted = aes_decrypt(
            &key,
            &self.ciphertext,
            &self.nonce,
            format!("lunote-note:{}", self.context).as_bytes(),
        );
        key.fill(0);
        decrypted.map_err(|_| anyhow!("密码错误或笔记已损坏"))
    }

    fn update(&self, plain: &[u8], password: &str) -> Result<Self> {
        let mut key = [0; 32];
        derive_kek(password, &self.salt, &mut key)?;
        let encrypted = aes_encrypt(
            &key,
            plain,
            format!("lunote-note:{}", self.context).as_bytes(),
        );
        key.fill(0);
        let (nonce, ciphertext) = encrypted?;
        Ok(Self {
            context: self.context.clone(),
            salt: self.salt.clone(),
            nonce,
            ciphertext,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub body: Option<String>,
    pub locked: Option<LockedBody>,
    pub clock: Clock,
    pub deleted: bool,
    pub pinned: bool,
    pub group: String,
    pub order: i64,
    pub shielded: bool,
}

impl Note {
    fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || self.id.len() > 128
            || self.title.len() > 256
            || self.group.len() > 128
            || self.clock.is_empty()
            || self.clock.len() > 64
            || self.clock.iter().any(|(k, v)| k.len() > 128 || *v == 0)
            || self.body.as_ref().is_some_and(|s| s.len() > 64 * 1024)
        {
            bail!("笔记格式或长度非法");
        }
        if let Some(locked) = &self.locked {
            if self.body.is_some()
                || locked.salt.len() != 16
                || locked.nonce.len() != 12
                || locked.ciphertext.len() > 65 * 1024
                || locked.context.len() > 128
            {
                bail!("加密笔记格式非法");
            }
        }
        Ok(())
    }

    fn fingerprint(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).expect("note serialization"))
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoteVersion {
    pub id: String,
    pub clock: Clock,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Workspace {
    notes: BTreeMap<String, Note>,
    peers: BTreeSet<String>,
    counter: u64,
}

pub struct Notes {
    store: Arc<Store>,
    local_id: String,
    state: Mutex<Workspace>,
}

impl Notes {
    pub fn open(store: Arc<Store>, local_id: String) -> Result<Arc<Self>> {
        let state = store.load_feature(STORAGE_KEY)?.unwrap_or_default();
        Ok(Arc::new(Self {
            store,
            local_id,
            state: Mutex::new(state),
        }))
    }

    pub fn peers(&self) -> Vec<String> {
        self.state.lock().unwrap().peers.iter().cloned().collect()
    }
    pub fn allows(&self, peer: &str) -> bool {
        self.state.lock().unwrap().peers.contains(peer)
    }
    pub fn select_peers(&self, peers: BTreeSet<String>) -> Result<()> {
        if peers.len() > 32 || peers.contains(&self.local_id) {
            bail!("同步设备设置非法");
        }
        let mut state = self.state.lock().unwrap();
        let mut next = state.clone();
        next.peers = peers;
        self.store.save_feature(STORAGE_KEY, &next)?;
        *state = next;
        Ok(())
    }
    pub fn list(&self) -> Vec<Note> {
        self.state.lock().unwrap().notes.values().cloned().collect()
    }
    pub fn versions(&self) -> Vec<NoteVersion> {
        self.list()
            .into_iter()
            .map(|n| NoteVersion {
                id: n.id,
                clock: n.clock,
            })
            .collect()
    }
    pub fn needed(&self, versions: &[NoteVersion]) -> Result<Vec<String>> {
        if versions.len() > MAX_NOTES
            || versions.iter().any(|v| {
                v.id.len() > 128 || v.clock.len() > 64 || v.clock.keys().any(|k| k.len() > 128)
            })
        {
            bail!("笔记同步索引过大");
        }
        let state = self.state.lock().unwrap();
        Ok(versions
            .iter()
            .filter(|v| {
                state
                    .notes
                    .get(&v.id)
                    .is_none_or(|n| !dominates(&n.clock, &v.clock))
            })
            .map(|v| v.id.clone())
            .collect())
    }

    pub fn unlock(&self, id: &str, password: &str) -> Result<String> {
        let note = self
            .state
            .lock()
            .unwrap()
            .notes
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("笔记不存在"))?;
        if note.deleted {
            bail!("笔记已删除");
        }
        match note.locked {
            Some(lock) => Ok(String::from_utf8(lock.open(password)?)?),
            None => Ok(note.body.unwrap_or_default()),
        }
    }

    /// The expected clock prevents an editor opened before a sync from overwriting it.
    pub fn save(
        &self,
        mut input: Note,
        password: Option<&str>,
        new_password: Option<&str>,
    ) -> Result<Note> {
        let mut state = self.state.lock().unwrap();
        let old = state.notes.get(&input.id).cloned();
        if let Some(old) = &old {
            if old.deleted || old.clock != input.clock {
                bail!("笔记已被另一设备修改，请重新打开后编辑");
            }
            input.locked = old.locked.clone();
            if let Some(lock) = &old.locked {
                if input.body.is_some() || new_password.is_some() {
                    let plain = String::from_utf8(
                        lock.open(password.ok_or_else(|| anyhow!("请输入笔记密码"))?)?,
                    )?;
                    let body = input.body.take().unwrap_or(plain);
                    if let Some(new) = new_password {
                        if new.is_empty() {
                            input.locked = None;
                            input.body = Some(body);
                        } else {
                            input.locked =
                                Some(LockedBody::seal(&lock.context, body.as_bytes(), new)?);
                        }
                    } else {
                        input.locked = Some(lock.update(body.as_bytes(), password.unwrap())?);
                    }
                }
            }
        } else {
            if state.notes.len() >= MAX_NOTES || !input.clock.is_empty() {
                bail!("无法创建笔记（数量已达上限或版本非法）");
            }
            input.id = crate::messages::new_id();
            input.locked = None;
        }
        if input.locked.is_none() {
            if let Some(new) = new_password.filter(|p| !p.is_empty()) {
                input.locked = Some(LockedBody::seal(
                    &input.id,
                    input.body.take().unwrap_or_default().as_bytes(),
                    new,
                )?);
            }
        }
        let mut next = state.clone();
        next.counter = next
            .counter
            .max(*input.clock.get(&self.local_id).unwrap_or(&0))
            .checked_add(1)
            .ok_or_else(|| anyhow!("笔记版本溢出"))?;
        input.clock.insert(self.local_id.clone(), next.counter);
        if input.deleted {
            input.body = None;
            input.locked = None;
            input.title.clear();
            input.group.clear();
            input.pinned = false;
            input.shielded = false;
        }
        input.validate()?;
        next.notes.insert(input.id.clone(), input.clone());
        self.store.save_feature(STORAGE_KEY, &next)?;
        *state = next;
        Ok(input)
    }

    pub fn merge(&self, remote: Vec<Note>) -> Result<bool> {
        if remote.len() > 16 {
            bail!("笔记同步批次过大");
        }
        self.merge_records(remote)
    }

    fn merge_records(&self, remote: Vec<Note>) -> Result<bool> {
        if remote.len() > MAX_NOTES {
            bail!("笔记列表过大");
        }
        for note in &remote {
            note.validate()?;
        }
        let mut state = self.state.lock().unwrap();
        let mut next = state.clone();
        let mut changed = false;
        for incoming in remote {
            let Some(local) = next.notes.get(&incoming.id).cloned() else {
                if next.notes.len() >= MAX_NOTES {
                    bail!("笔记数量已达上限");
                }
                next.notes.insert(incoming.id.clone(), incoming);
                changed = true;
                continue;
            };
            if local == incoming {
                continue;
            }
            let local_after = dominates(&local.clock, &incoming.clock);
            let remote_after = dominates(&incoming.clock, &local.clock);
            if local_after && !remote_after {
                continue;
            }
            if remote_after && !local_after {
                next.notes.insert(incoming.id.clone(), incoming);
                changed = true;
                continue;
            }
            // Concurrent edits converge deterministically, while retaining the losing content.
            let mut clock = local.clock.clone();
            for (device, n) in &incoming.clock {
                clock
                    .entry(device.clone())
                    .and_modify(|v| *v = (*v).max(*n))
                    .or_insert(*n);
            }
            let remote_wins = if local.deleted != incoming.deleted {
                incoming.deleted
            } else {
                incoming.fingerprint() > local.fingerprint()
            };
            let (mut winner, mut conflict) = if remote_wins {
                (incoming, local)
            } else {
                (local, incoming)
            };
            if !conflict.deleted {
                if next.notes.len() >= MAX_NOTES {
                    bail!("无法保存冲突副本，笔记数量已达上限");
                }
                conflict.id = format!("conflict:{}", conflict.fingerprint());
                conflict.title = truncate_title(&format!("{}（冲突副本）", conflict.title));
                conflict.clock = clock.clone();
                next.notes.insert(conflict.id.clone(), conflict);
            }
            winner.clock = clock;
            next.notes.insert(winner.id.clone(), winner);
            changed = true;
        }
        if changed {
            self.store.save_feature(STORAGE_KEY, &next)?;
            *state = next;
        }
        Ok(changed)
    }

    pub fn export(&self, password: &str, path: &Path) -> Result<()> {
        let backup = LockedBody::seal(
            "notes-backup-v1",
            &serde_json::to_vec(&self.list())?,
            password,
        )?;
        crate::platform::atomic_write(path, &serde_json::to_vec(&backup)?, true)
    }

    pub fn reorder(&self, versions: Vec<NoteVersion>) -> Result<()> {
        if versions.len() > MAX_NOTES {
            bail!("排序列表过大");
        }
        let mut state = self.state.lock().unwrap();
        let mut next = state.clone();
        let mut seen = BTreeSet::new();
        for (index, version) in versions.into_iter().enumerate() {
            if !seen.insert(version.id.clone()) {
                bail!("排序包含重复笔记");
            }
            let note = next
                .notes
                .get_mut(&version.id)
                .ok_or_else(|| anyhow!("笔记不存在"))?;
            if note.deleted || note.clock != version.clock {
                bail!("笔记已更新，请重试排序");
            }
            let order = index as i64;
            if note.order != order {
                next.counter = next
                    .counter
                    .max(*note.clock.get(&self.local_id).unwrap_or(&0))
                    .checked_add(1)
                    .ok_or_else(|| anyhow!("版本溢出"))?;
                note.order = order;
                note.clock.insert(self.local_id.clone(), next.counter);
            }
        }
        self.store.save_feature(STORAGE_KEY, &next)?;
        *state = next;
        Ok(())
    }

    pub fn import(&self, password: &str, path: &Path) -> Result<()> {
        if std::fs::metadata(path)?.len() > 128 * 1024 * 1024 {
            bail!("备份文件过大");
        }
        let backup: LockedBody = serde_json::from_slice(&std::fs::read(path)?)?;
        if backup.context != "notes-backup-v1" {
            bail!("不是笔记备份");
        }
        let notes: Vec<Note> = serde_json::from_slice(&backup.open(password)?)?;
        if notes.len() > MAX_NOTES {
            bail!("备份笔记过多");
        }
        for note in &notes {
            note.validate()?;
        }
        self.merge_records(notes)?;
        Ok(())
    }
}

fn dominates(a: &Clock, b: &Clock) -> bool {
    b.iter()
        .all(|(id, n)| a.get(id).copied().unwrap_or(0) >= *n)
}
fn truncate_title(s: &str) -> String {
    let mut end = s.len().min(256);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draft() -> Note {
        Note {
            id: String::new(),
            title: "private-title".into(),
            body: Some("private-api-key".into()),
            locked: None,
            clock: Clock::new(),
            deleted: false,
            pinned: false,
            group: String::new(),
            order: 0,
            shielded: false,
        }
    }
    #[test]
    fn locked_notes_and_backup_require_password() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let notes = Notes::open(store.clone(), "a".into()).unwrap();
        let saved = notes.save(draft(), None, Some("strong-pass-123")).unwrap();
        assert!(saved.body.is_none());
        assert_eq!(
            notes.unlock(&saved.id, "strong-pass-123").unwrap(),
            "private-api-key"
        );
        assert!(notes.unlock(&saved.id, "wrong-password").is_err());
        let backup = dir.path().join("backup.lunotes");
        notes.export("backup-password", &backup).unwrap();
        let restored_dir = tempfile::tempdir().unwrap();
        let restored = Notes::open(
            Arc::new(Store::open(restored_dir.path()).unwrap()),
            "b".into(),
        )
        .unwrap();
        assert!(restored.import("wrong-password", &backup).is_err());
        restored.import("backup-password", &backup).unwrap();
        assert_eq!(
            restored.unlock(&saved.id, "strong-pass-123").unwrap(),
            "private-api-key"
        );
        for file in ["records.db", "records.db-wal", "backup.lunotes"] {
            if let Ok(bytes) = std::fs::read(dir.path().join(file)) {
                let text = String::from_utf8_lossy(&bytes);
                assert!(!text.contains("private-api-key"));
                assert!(!text.contains("strong-pass-123"));
                assert!(!text.contains("private-title"));
            }
        }
        let reopened = Notes::open(store, "a".into()).unwrap();
        assert_eq!(reopened.list(), notes.list());
    }
    #[test]
    fn concurrent_changes_converge_without_losing_content() {
        let da = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let a = Notes::open(Arc::new(Store::open(da.path()).unwrap()), "a".into()).unwrap();
        let b = Notes::open(Arc::new(Store::open(db.path()).unwrap()), "b".into()).unwrap();
        let base = a.save(draft(), None, None).unwrap();
        b.merge(vec![base.clone()]).unwrap();
        let mut left = base.clone();
        left.body = Some("edit-a".into());
        let mut right = base.clone();
        right.body = Some("edit-b".into());
        let left = a.save(left, None, None).unwrap();
        let right = b.save(right, None, None).unwrap();
        a.merge(vec![right]).unwrap();
        b.merge(vec![left]).unwrap();
        a.merge(b.list()).unwrap();
        b.merge(a.list()).unwrap();
        assert_eq!(a.list(), b.list());
        assert_eq!(a.list().len(), 2);
        assert!(a.list().iter().any(|n| n.body.as_deref() == Some("edit-a")));
        assert!(a.list().iter().any(|n| n.body.as_deref() == Some("edit-b")));
        assert!(
            a.save(base, None, None).is_err(),
            "stale editor overwrote newer note"
        );
    }

    #[test]
    fn password_changes_and_deletion_propagate_without_plaintext() {
        let da = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let a = Notes::open(Arc::new(Store::open(da.path()).unwrap()), "a".into()).unwrap();
        let b = Notes::open(Arc::new(Store::open(db.path()).unwrap()), "b".into()).unwrap();
        let base = a.save(draft(), None, Some("password-old")).unwrap();
        b.merge(vec![base.clone()]).unwrap();
        let old_salt = base.locked.as_ref().unwrap().salt.clone();
        let mut edited = base.clone();
        edited.body = Some("edited-key".into());
        let edited = a.save(edited, Some("password-old"), None).unwrap();
        assert_eq!(
            edited.locked.as_ref().unwrap().salt,
            old_salt,
            "normal editing invalidated local system credentials"
        );
        let changed = a
            .save(edited, Some("password-old"), Some("password-new"))
            .unwrap();
        assert_ne!(changed.locked.as_ref().unwrap().salt, old_salt);
        b.merge(vec![changed.clone()]).unwrap();
        assert!(b.unlock(&base.id, "password-old").is_err());
        assert_eq!(b.unlock(&base.id, "password-new").unwrap(), "edited-key");
        let mut deleted = changed;
        deleted.deleted = true;
        let deleted = a.save(deleted, None, None).unwrap();
        assert!(deleted.body.is_none() && deleted.locked.is_none() && deleted.title.is_empty());
        b.merge(vec![deleted]).unwrap();
        b.merge(vec![base]).unwrap();
        assert!(b.list()[0].deleted);
        assert!(b.unlock(&b.list()[0].id, "password-new").is_err());
    }
}
