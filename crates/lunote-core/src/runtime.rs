//! Runtime 聚合：生命周期、事件流、面向 UI/CLI 的完整 API。
//!
//! 边界：UI/CLI 只能通过 Runtime 操作核心；设备列表、传输状态、消息全部来自
//! 核心事件或查询接口，不维护任何“假数据”。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use crate::discovery::{Discovery, DiscoveryConfig, PeerAddr};
use crate::events::{CoreEvent, EventBus};
use crate::identity::DeviceIdentity;
use crate::session::SessionManager;
use crate::store::{ExportReport, ImportReport, Store, StoredConversation};
use crate::transfer::{ConflictPolicy, SendFile, TransferManager};
use crate::trust::{TrustRecord, TrustStore};

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub data_dir: PathBuf,
    pub name: String,
    pub discovery_port: u16,
    pub tcp_port: u16,
    /// 发现信标间隔（默认 1s；后台节流时乘以倍率）
    pub discovery_interval: std::time::Duration,
    /// 离线判定超时（默认 4s）
    pub offline_timeout: std::time::Duration,
    /// 接收文件默认目录（默认 data_dir/downloads）
    pub downloads_dir: Option<PathBuf>,
    pub background: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("data"),
            name: "我的设备".into(),
            discovery_port: crate::discovery::DEFAULT_PORT,
            tcp_port: 45455,
            discovery_interval: std::time::Duration::from_secs(1),
            offline_timeout: std::time::Duration::from_secs(4),
            downloads_dir: None,
            background: false,
        }
    }
}

pub struct Runtime {
    pub identity: Arc<DeviceIdentity>,
    pub bus: EventBus,
    pub trust: Arc<Mutex<TrustStore>>,
    pub store: Arc<Store>,
    pub discovery: Arc<Discovery>,
    pub sessions: Arc<SessionManager>,
    pub transfers: Arc<TransferManager>,
    pub downloads_dir: PathBuf,
    pub tcp_port: u16,
    pub auto_trust: Arc<AtomicBool>,
    /// 用户自定义接收目录（settings.json 持久化；None=用默认 downloads_dir）
    pub downloads_dir_setting: Mutex<Option<PathBuf>>,
    /// 主题（"dark"/"light"/"system"；settings.json 持久化，默认 "dark"）
    pub theme_setting: Mutex<String>,
    pub conflict_setting: Mutex<ConflictPolicy>,
    pub receive_tree_uri: Mutex<Option<String>>,
    pub pin_hash: Mutex<Option<String>>,
    pub device_meta: Mutex<serde_json::Map<String, serde_json::Value>>,
    settings_write_lock: Mutex<()>,
    data_dir: PathBuf,
    rx: broadcast::Receiver<CoreEvent>,
    stop: Arc<AtomicBool>,
}

impl Runtime {
    pub async fn start(cfg: RuntimeConfig) -> Result<Self> {
        std::fs::create_dir_all(&cfg.data_dir)
            .with_context(|| format!("创建数据目录失败: {}", cfg.data_dir.display()))?;
        // 文件日志（诊断）：data_dir/core.log，滚动保留 core.1.log
        init_file_logging(&cfg.data_dir);
        let identity = Arc::new(DeviceIdentity::load_or_create(&cfg.data_dir, &cfg.name)?);
        let (bus, rx) = EventBus::new();
        let trust = Arc::new(Mutex::new(TrustStore::load_or_create(
            &cfg.data_dir.join("trust.json"),
        )?));
        let store = Arc::new(Store::open(&cfg.data_dir)?);
        let downloads_dir = cfg
            .downloads_dir
            .clone()
            .unwrap_or_else(|| cfg.data_dir.join("downloads"));

        // 设置只读一次，避免启动路径出现彼此覆盖的重复初始化。
        let mut auto_trust_enabled = true;
        let mut downloads_dir_setting: Option<PathBuf> = None;
        let mut theme_setting = "dark".to_string();
        let mut conflict_setting = ConflictPolicy::Rename;
        let mut receive_tree_uri: Option<String> = None;
        let mut pin_hash: Option<String> = None;
        let mut device_meta = serde_json::Map::new();
        if let Ok(data) = std::fs::read_to_string(cfg.data_dir.join("settings.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(b) = v.get("auto_trust").and_then(|x| x.as_bool()) {
                    auto_trust_enabled = b;
                }
                if let Some(s) = v.get("downloads_dir").and_then(|x| x.as_str()) {
                    if !s.is_empty() {
                        downloads_dir_setting = Some(PathBuf::from(s));
                    }
                }
                if let Some(t) = v.get("theme").and_then(|x| x.as_str()) {
                    if matches!(t, "dark" | "light" | "system" | "glass") {
                        theme_setting = t.to_string();
                    }
                }
                if let Some(c) = v.get("conflict").and_then(|x| x.as_str()) {
                    conflict_setting = match c {
                        "overwrite" => ConflictPolicy::Overwrite,
                        "skip" => ConflictPolicy::Skip,
                        _ => ConflictPolicy::Rename,
                    };
                }
                receive_tree_uri = v
                    .get("receive_tree_uri")
                    .and_then(|x| x.as_str())
                    .map(str::to_owned);
                pin_hash = v
                    .get("pin_hash")
                    .and_then(|x| x.as_str())
                    .map(str::to_owned);
                if let Some(m) = v.get("device_meta").and_then(|x| x.as_object()) {
                    device_meta = m.clone();
                }
            }
        }
        let auto_trust = Arc::new(AtomicBool::new(auto_trust_enabled));
        let downloads_dir_setting = Mutex::new(downloads_dir_setting);
        let theme_setting = Mutex::new(theme_setting);

        let discovery_cfg = DiscoveryConfig {
            port: cfg.discovery_port,
            interval: cfg.discovery_interval,
            timeout: cfg.offline_timeout,
            ..Default::default()
        };
        let discovery = Discovery::new(discovery_cfg, bus.clone(), identity.clone(), cfg.tcp_port)
            .await
            .context("发现服务初始化失败")?;
        let transfers = TransferManager::new(
            bus.clone(),
            store.clone(),
            trust.clone(),
            downloads_dir.clone(),
        )?;
        transfers.set_conflict_policy(conflict_setting);
        let sessions = SessionManager::new(
            identity.clone(),
            trust.clone(),
            store.clone(),
            bus.clone(),
            discovery.clone(),
            transfers.clone(),
            auto_trust.clone(),
        )?;
        transfers.set_session(sessions.clone());
        sessions.start(cfg.tcp_port).await?;
        discovery.start();
        if cfg.background {
            discovery.set_background(true);
        }

        // 事件 → 会话名同步（保持记录中的对端名称最新）
        // 注意：不在此处更新 trust.last_ip——发现信标无认证，伪造信标可覆写
        // last_ip 并配合“同名同 IP 自动信任”提升攻击面；last_ip 只在
        // TLS+签名握手验证成功后由 session.rs 更新（连接真实来源）。
        let store2 = store.clone();
        let mut ev_rx = bus.subscribe();
        tokio::spawn(async move {
            while let Ok(ev) = ev_rx.recv().await {
                match &ev {
                    CoreEvent::PeerOnline {
                        device_id, name, ..
                    }
                    | CoreEvent::PeerConnected {
                        device_id, name, ..
                    } => {
                        let _ = store2.update_conversation_name(device_id, name);
                    }
                    _ => {}
                }
            }
        });

        Ok(Self {
            identity,
            bus,
            trust,
            store,
            discovery,
            sessions,
            transfers,
            downloads_dir,
            tcp_port: cfg.tcp_port,
            auto_trust,
            downloads_dir_setting,
            theme_setting,
            conflict_setting: Mutex::new(conflict_setting),
            receive_tree_uri: Mutex::new(receive_tree_uri),
            pin_hash: Mutex::new(pin_hash),
            device_meta: Mutex::new(device_meta),
            settings_write_lock: Mutex::new(()),
            data_dir: cfg.data_dir,
            rx,
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn events(&self) -> broadcast::Receiver<CoreEvent> {
        self.bus.subscribe()
    }

    /// 丢弃初始订阅（用于 CLI 一次性查询）
    pub fn clear_event_queue(&mut self) {
        while self.rx.try_recv().is_ok() {}
    }

    // ---------- 设备与信任 ----------

    pub fn peers(&self) -> Vec<PeerAddr> {
        self.discovery.peers_snapshot()
    }

    pub fn trust_list(&self) -> Vec<TrustRecord> {
        self.trust.lock().unwrap().list()
    }

    pub fn is_trusted(&self, device_id: &str) -> bool {
        self.trust.lock().unwrap().is_trusted(device_id)
    }

    pub fn trust_device(&self, device_id: &str, name: &str, trusted: bool) -> Result<()> {
        let mut trust = self.trust.lock().unwrap();
        // 无 TOFU 记录也可信任（设备页直接点信任；指纹待首次连接确认）
        trust.trust_device(device_id, name, trusted)?;
        let name = trust
            .get(device_id)
            .map(|r| r.name.clone())
            .unwrap_or_default();
        drop(trust);
        self.bus.emit(CoreEvent::TrustChanged {
            device_id: device_id.to_string(),
            name,
            trusted,
        });
        Ok(())
    }

    pub fn remove_device(&self, device_id: &str) -> Result<()> {
        self.trust.lock().unwrap().remove(device_id)
    }

    pub fn fingerprint_of(&self, device_id: &str) -> Option<String> {
        self.trust
            .lock()
            .unwrap()
            .get(device_id)
            .map(|r| r.fingerprint.clone())
    }

    pub fn rename_device(&self, name: &str) -> Result<()> {
        self.identity.set_name(name)
    }

    /// 主动连接设备（发送前自动调用；也可由 UI 手动触发）
    pub async fn connect_to(&self, device_id: &str) -> Result<()> {
        self.sessions.connect_to(device_id).await
    }

    // ---------- 消息 ----------

    pub async fn send_text(&self, device_id: &str, text: &str) -> Result<String> {
        self.sessions.send_text(device_id, text).await
    }

    pub async fn send_link(
        &self,
        device_id: &str,
        url: &str,
        title: Option<&str>,
    ) -> Result<String> {
        self.sessions.send_link(device_id, url, title).await
    }

    // ---------- 文件 ----------

    /// 发送文件/文件夹（自动展开目录为相对路径清单，逐文件流式传输）
    pub async fn send_paths(&self, device_id: &str, paths: Vec<PathBuf>) -> Result<Vec<String>> {
        let mut files = Vec::new();
        for p in &paths {
            let meta = tokio::fs::metadata(p).await?;
            if meta.is_file() {
                files.push(SendFile {
                    path: p.clone(),
                    rel_parts: vec![],
                });
            } else if meta.is_dir() {
                collect_dir(p, p, &mut files)?;
            } else {
                anyhow::bail!("不支持的路径类型: {}", p.display());
            }
        }
        self.transfers.send_files(device_id, files).await
    }

    pub async fn accept_transfer(&self, transfer_id: &str, dest_dir: &Path) -> Result<()> {
        self.transfers.accept_transfer(transfer_id, dest_dir).await
    }

    pub async fn reject_transfer(&self, transfer_id: &str, reason: &str) -> Result<()> {
        self.transfers.reject_transfer(transfer_id, reason).await
    }

    pub async fn cancel_transfer(&self, transfer_id: &str) -> Result<()> {
        self.transfers.cancel_transfer(transfer_id).await
    }

    pub async fn pause_transfer(&self, transfer_id: &str) -> Result<()> {
        self.transfers.pause_transfer(transfer_id).await
    }

    pub async fn resume_transfer(&self, transfer_id: &str) -> Result<()> {
        self.transfers.resume_transfer(transfer_id).await
    }

    pub fn set_conflict_policy(&self, policy: &str) -> Result<()> {
        let p = match policy {
            "overwrite" => ConflictPolicy::Overwrite,
            "skip" => ConflictPolicy::Skip,
            _ => ConflictPolicy::Rename,
        };
        self.transfers.set_conflict_policy(p);
        *self.conflict_setting.lock().unwrap() = p;
        self.write_settings(serde_json::json!({"conflict": policy}))
    }

    pub fn set_receive_tree_uri(&self, uri: Option<&str>) -> Result<()> {
        *self.receive_tree_uri.lock().unwrap() = uri.map(str::to_owned);
        self.write_settings(serde_json::json!({"receive_tree_uri": uri}))
    }

    pub fn set_pin(&self, pin: Option<&str>) -> Result<()> {
        let hash = pin.map(|p| {
            let mut h = Sha256::new();
            h.update(p.as_bytes());
            format!("{:x}", h.finalize())
        });
        *self.pin_hash.lock().unwrap() = hash.clone();
        self.write_settings(serde_json::json!({"pin_hash": hash}))
    }

    pub fn verify_pin(&self, pin: &str) -> bool {
        let mut h = Sha256::new();
        h.update(pin.as_bytes());
        let candidate = format!("{:x}", h.finalize());
        self.pin_hash.lock().unwrap().as_deref() == Some(candidate.as_str())
    }

    pub fn set_device_meta(
        &self,
        device_id: &str,
        alias: Option<&str>,
        favorite: Option<bool>,
    ) -> Result<()> {
        let mut meta = self.device_meta.lock().unwrap();
        let entry = meta
            .entry(device_id.to_string())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(obj) = entry.as_object_mut() {
            if let Some(a) = alias {
                obj.insert("alias".into(), serde_json::Value::String(a.to_string()));
            }
            if let Some(f) = favorite {
                obj.insert("favorite".into(), serde_json::Value::Bool(f));
            }
        }
        let snapshot = meta.clone();
        drop(meta);
        self.write_settings(serde_json::json!({"device_meta": snapshot}))
    }

    pub fn transfers(&self) -> Vec<crate::events::TransferInfo> {
        self.transfers.list()
    }

    // ---------- 记录 ----------

    pub fn conversations(&self) -> Result<Vec<StoredConversation>> {
        self.store.list_conversations()
    }

    pub fn export_records(&self, password: &str, out_path: &Path) -> Result<ExportReport> {
        self.store.export(password, out_path)
    }

    pub fn import_records(&self, password: &str, in_path: &Path) -> Result<ImportReport> {
        self.store.import(password, in_path)
    }

    pub fn wipe_records(&self) -> Result<()> {
        self.store.wipe()
    }

    /// 删除与某设备的整个对话（本地记录）；不影响信任与身份
    pub fn delete_conversation(&self, device_id: &str) -> Result<()> {
        self.store.delete_conversation(device_id)?;
        self.bus.emit(CoreEvent::RecordsChanged);
        Ok(())
    }

    pub fn delete_conversations(&self, device_ids: &[String]) -> Result<()> {
        self.store.delete_conversations(device_ids)?;
        self.bus.emit(CoreEvent::RecordsChanged);
        Ok(())
    }

    // ---------- 后台节流 ----------

    pub fn set_background(&self, bg: bool) {
        self.discovery.set_background(bg);
    }

    // ---------- 同名同 IP 自动信任 ----------

    pub fn auto_trust_enabled(&self) -> bool {
        self.auto_trust.load(Ordering::Relaxed)
    }

    /// 读取 settings.json 当前各项（供 UI 拉取）
    pub fn settings(&self) -> serde_json::Value {
        serde_json::json!({
            "auto_trust": self.auto_trust_enabled(),
            "downloads_dir": self.downloads_dir_setting.lock().unwrap().as_ref().map(|p| p.to_string_lossy().to_string()),
            "theme": self.theme_setting.lock().unwrap().clone(),
            "conflict": match *self.conflict_setting.lock().unwrap() { ConflictPolicy::Rename => "rename", ConflictPolicy::Overwrite => "overwrite", ConflictPolicy::Skip => "skip" },
            "receive_tree_uri": self.receive_tree_uri.lock().unwrap().clone(),
            "pin_enabled": self.pin_hash.lock().unwrap().is_some(),
            "device_meta": self.device_meta.lock().unwrap().clone(),
        })
    }

    /// 写 settings.json（合并保留其它字段）
    fn write_settings(&self, patch: serde_json::Value) -> Result<()> {
        let _guard = self.settings_write_lock.lock().unwrap();
        let path = self.data_dir.join("settings.json");
        let mut cur = serde_json::json!({
            "auto_trust": self.auto_trust_enabled(),
            "downloads_dir": self.downloads_dir_setting.lock().unwrap().as_ref().map(|p| p.to_string_lossy().to_string()),
            "theme": self.theme_setting.lock().unwrap().clone(),
            "conflict": match *self.conflict_setting.lock().unwrap() { ConflictPolicy::Rename => "rename", ConflictPolicy::Overwrite => "overwrite", ConflictPolicy::Skip => "skip" },
            "receive_tree_uri": self.receive_tree_uri.lock().unwrap().clone(),
            "pin_enabled": self.pin_hash.lock().unwrap().is_some(),
            "device_meta": self.device_meta.lock().unwrap().clone(),
        });
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(o) = v.as_object() {
                    for (k, val) in o {
                        cur[k] = val.clone();
                    }
                }
            }
        }
        if let Some(obj) = patch.as_object() {
            for (k, val) in obj {
                cur[k] = val.clone();
            }
        }
        let data = serde_json::to_string(&cur)?;
        crate::platform::atomic_write(&path, data.as_bytes(), false)
            .context("settings.json 原子写入失败")
    }

    pub fn set_auto_trust(&self, enabled: bool) -> Result<()> {
        let previous = self.auto_trust.swap(enabled, Ordering::Relaxed);
        if let Err(error) = self.write_settings(serde_json::json!({ "auto_trust": enabled })) {
            self.auto_trust.store(previous, Ordering::Relaxed);
            return Err(error);
        }
        Ok(())
    }

    /// 设置自定义接收目录；None 恢复为默认（data/downloads）
    pub fn set_downloads_dir(&self, dir: Option<&str>) -> Result<()> {
        let d = dir.map(PathBuf::from);
        let previous = {
            let mut setting = self.downloads_dir_setting.lock().unwrap();
            std::mem::replace(&mut *setting, d.clone())
        };
        let result = self.write_settings(serde_json::json!({
            "downloads_dir": d.as_ref().map(|p| p.to_string_lossy().to_string()),
        }));
        if let Err(error) = result {
            *self.downloads_dir_setting.lock().unwrap() = previous;
            return Err(error);
        }
        Ok(())
    }

    /// 设置主题：dark / light / system / glass（持久化）
    pub fn set_theme(&self, theme: &str) -> Result<()> {
        let t = if matches!(theme, "dark" | "light" | "system" | "glass") {
            theme.to_string()
        } else {
            "dark".to_string()
        };
        let previous = {
            let mut setting = self.theme_setting.lock().unwrap();
            std::mem::replace(&mut *setting, t.clone())
        };
        if let Err(error) = self.write_settings(serde_json::json!({ "theme": t })) {
            *self.theme_setting.lock().unwrap() = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn discovery_stats(&self) -> crate::discovery::DiscoveryStats {
        self.discovery.stats()
    }

    pub fn diagnostics(&self) -> serde_json::Value {
        let stats = self.discovery_stats();
        serde_json::json!({
            "device_id": self.identity.device_id,
            "device_name": self.identity.name(),
            "tcp_port": self.tcp_port,
            "peers_online": self.peers().iter().filter(|p| p.online).count(),
            "peers_total": self.peers().len(),
            "discovery": stats,
            "data_dir": self.data_dir.to_string_lossy(),
            "downloads_dir": self.downloads_dir_setting.lock().unwrap().as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| self.downloads_dir.to_string_lossy().to_string()),
        })
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.discovery.shutdown();
        self.sessions.shutdown();
    }
}

/// 每次写入后立即刷盘的文件 writer：保证“一键导出诊断日志”永远读到最新内容
struct FlushLogWriter(std::fs::File);

impl std::io::Write for FlushLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.0.write(buf)?;
        let _ = self.0.flush();
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

/// 初始化文件日志（诊断）：data_dir/core.log，超过 2MB 滚动为 core.1.log
fn init_file_logging(data_dir: &Path) {
    let path = data_dir.join("core.log");
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 2 * 1024 * 1024 {
            let _ = std::fs::rename(&path, data_dir.join("core.1.log"));
        }
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(FlushLogWriter(file)))
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .try_init();
    }
}

/// 展开目录为文件清单。防御：跳过符号链接（防指向祖先目录造成无限递归），
/// 文件总数上限 5000（防误选超大目录拖垮发送）。
fn collect_dir(root: &Path, dir: &Path, out: &mut Vec<SendFile>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue; // 符号链接一律跳过（不跟随、不递归）
        }
        if ft.is_dir() {
            collect_dir(root, &path, out)?;
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        if out.len() >= 5000 {
            anyhow::bail!("文件夹内文件过多（超过 5000 个），已中止发送");
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| anyhow!("路径关系错误"))?;
        let mut rel_parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        // 相对路径只保留目录组件，文件名由 FileOffer.name 携带
        if !rel_parts.is_empty() {
            rel_parts.pop();
        }
        out.push(SendFile { path, rel_parts });
    }
    Ok(())
}
