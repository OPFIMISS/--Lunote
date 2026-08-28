//! 局域网自动发现（LUNOTE1 协议，迁移自 lanprobe 已验证的 PROBE1 设计）。
//!
//! 发送：UDP 组播 239.255.77.77:45454（TTL=1，逐接口）+ 有限广播 255.255.255.255
//!       + 各接口子网定向广播。
//! 报文：`LUNOTE1|ver=1|name=<百分号编码>|dev=<设备ID>|inst=<实例ID>|seq=<序号>|ts=<毫秒>|port=<TCP端口>`
//!
//! 与探针相比的正式化增强：
//! - 版本字段校验（未知版本 → 忽略并记录，不误判为设备）；
//! - 错误/畸形数据包统计与忽略；
//! - seq 去重（重复数据包）；
//! - 设备重启（同 device_id 新 instance_id → 替换旧实例）；
//! - 同名设备按 device_id 区分，不合并；
//! - 多网卡逐接口组播；同一设备多接口出现时以最新实例为准；
//! - 改名（同实例名称变化 → 更新，身份不变）；
//! - 离线超时与后台节流（降低频率，保留接收能力）。

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rand::RngCore;
use socket2::SockRef;
use tokio::net::UdpSocket;

use crate::events::{CoreEvent, EventBus};
use crate::identity::DeviceIdentity;

pub const PROTO_NAME: &str = "LUNOTE1";
pub const PROTO_VER: u64 = 1;
pub const DEFAULT_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 77, 77);
pub const DEFAULT_BROADCAST: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 255);
pub const DEFAULT_PORT: u16 = 45454;

#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    pub port: u16,
    pub group: Ipv4Addr,
    pub broadcast: Ipv4Addr,
    pub interval: Duration,
    pub timeout: Duration,
    pub subnet_broadcast: bool,
    /// 后台节流倍率（>1 时降低发送频率）
    pub background_multiplier: f32,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            group: DEFAULT_GROUP,
            broadcast: DEFAULT_BROADCAST,
            interval: Duration::from_secs(1),
            timeout: Duration::from_secs(4),
            subnet_broadcast: true,
            background_multiplier: 3.0,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PeerAddr {
    pub device_id: String,
    pub instance_id: String,
    pub name: String,
    pub ip: String,
    pub tcp_port: u16,
    pub online: bool,
}

#[derive(Clone, Debug)]
struct PeerEntry {
    device_id: String,
    instance_id: String,
    name: String,
    ip: String,
    tcp_port: u16,
    last_seq: u64,
    last_seen: Instant,
    first_seen_ms: i64,
    online: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DiscoveryStats {
    pub tx_packets: u64,
    pub tx_failures: u64,
    pub rx_total: u64,
    pub rx_valid: u64,
    pub rx_self: u64,
    pub rx_dup: u64,
    pub rx_invalid: u64,
}

#[derive(Clone, Debug)]
struct Iface {
    name: String,
    ip: Ipv4Addr,
    broadcast: Option<Ipv4Addr>,
}

pub struct Discovery {
    cfg: DiscoveryConfig,
    bus: EventBus,
    identity: Arc<DeviceIdentity>,
    tcp_port: u16,
    sock: Arc<UdpSocket>,
    ifaces: Vec<Iface>,
    peers: Arc<Mutex<HashMap<String, PeerEntry>>>, // instance_id -> entry
    by_device: Arc<Mutex<HashMap<String, PeerAddr>>>, // device_id -> 最新地址
    stats: Arc<Mutex<DiscoveryStats>>,
    background: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    instance_id: String,
}

fn local_ifaces() -> Vec<Iface> {
    let mut out = Vec::new();
    let Ok(ifaddrs) = if_addrs::get_if_addrs() else {
        return out;
    };
    for item in ifaddrs {
        let if_addrs::IfAddr::V4(v4) = item.addr else {
            continue;
        };
        let ip = v4.ip;
        if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_multicast() {
            continue;
        }
        let broadcast = match v4.broadcast {
            Some(b) if !b.is_unspecified() => Some(b),
            _ => None,
        };
        out.push(Iface {
            name: item.name.clone(),
            ip,
            broadcast,
        });
    }
    out
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for b in input.as_bytes() {
        let ok = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_'
                    | b'.'
                    | b'~'
                    | b' '
                    | b'#'
                    | b'&'
                    | b'@'
                    | b'$'
                    | b'%'
                    | b'^'
                    | b'*'
                    | b'('
                    | b')'
            );
        if ok {
            out.push(*b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // 过滤控制字符（NUL/换行等），防止污染事件、信任库与日志
    let decoded = String::from_utf8_lossy(&out);
    decoded
        .chars()
        .filter(|c| !c.is_control() && *c != '\u{FFFD}')
        .collect()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

struct ParsedBeacon {
    ver: u64,
    name: String,
    device_id: String,
    instance_id: String,
    seq: u64,
    ts: i64,
    port: u16,
}

fn parse_beacon(data: &[u8]) -> Option<ParsedBeacon> {
    if data.len() > 2048 {
        return None;
    }
    let text = String::from_utf8_lossy(data);
    let text = text.trim_matches(|c| c == '\0' || c == '\r' || c == '\n' || c == '\t' || c == ' ');
    let prefix = format!("{}|", PROTO_NAME);
    if !text.starts_with(&prefix) {
        return None;
    }
    let mut kv = std::collections::HashMap::new();
    for part in text[prefix.len()..].split('|') {
        if let Some((k, v)) = part.split_once('=') {
            kv.insert(k, v);
        }
    }
    let get = |k: &str| kv.get(k).map(|s| s.to_string());
    let (
        Some(ver),
        Some(name),
        Some(device_id),
        Some(instance_id),
        Some(seq),
        Some(ts),
        Some(port),
    ) = (
        get("ver"),
        get("name"),
        get("dev"),
        get("inst"),
        get("seq"),
        get("ts"),
        get("port"),
    )
    else {
        return None;
    };
    let (Ok(ver), Ok(seq), Ok(ts), Ok(port)) = (
        ver.parse::<u64>(),
        seq.parse::<u64>(),
        ts.parse::<i64>(),
        port.parse::<u16>(),
    ) else {
        return None;
    };
    if ver != PROTO_VER {
        return None; // 未知版本：忽略（调用方记录原因）
    }
    if port == 0 || !(1..=65535).contains(&port) {
        return None;
    }
    let name = percent_decode(&name);
    if name.is_empty()
        || name.len() > 256
        || device_id.is_empty()
        || device_id.len() > 128
        || instance_id.is_empty()
        || instance_id.len() > 128
    {
        return None;
    }
    Some(ParsedBeacon {
        ver,
        name,
        device_id,
        instance_id,
        seq,
        ts,
        port,
    })
}

impl Discovery {
    pub async fn new(
        cfg: DiscoveryConfig,
        bus: EventBus,
        identity: Arc<DeviceIdentity>,
        tcp_port: u16,
    ) -> Result<Arc<Self>> {
        let ifaces = local_ifaces();
        let std_sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .context("创建 UDP socket 失败")?;
        std_sock.set_reuse_address(true)?;
        std_sock.set_broadcast(true)?;
        std_sock.set_multicast_ttl_v4(1)?;
        std_sock.set_multicast_loop_v4(true)?;
        std_sock.set_nonblocking(true)?;
        std_sock
            .bind(&SocketAddr::from(([0, 0, 0, 0], cfg.port)).into())
            .with_context(|| format!("绑定发现端口 {} 失败（可能被占用）", cfg.port))?;
        let mut joined = Vec::new();
        let mut join_ips: Vec<Ipv4Addr> = ifaces.iter().map(|i| i.ip).collect();
        if join_ips.is_empty() {
            join_ips.push(Ipv4Addr::UNSPECIFIED);
        }
        for ip in &join_ips {
            match std_sock.join_multicast_v4(&cfg.group, ip) {
                Ok(()) => joined.push(*ip),
                Err(e) => tracing::warn!("加入组播失败 group={} via={} err={}", cfg.group, ip, e),
            }
        }
        let sock: std::net::UdpSocket = std_sock.into();
        let sock = Arc::new(UdpSocket::from_std(sock)?);
        let instance_id = uuid::Uuid::new_v4().to_string();
        tracing::info!(
            "发现服务启动 group={} port={} ifaces={} joined={} instance={}",
            cfg.group,
            cfg.port,
            ifaces.len(),
            joined.len(),
            instance_id
        );
        Ok(Arc::new(Self {
            cfg,
            bus,
            identity,
            tcp_port,
            sock,
            ifaces,
            peers: Arc::new(Mutex::new(HashMap::new())),
            by_device: Arc::new(Mutex::new(HashMap::new())),
            stats: Arc::new(Mutex::new(DiscoveryStats::default())),
            background: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
            instance_id,
        }))
    }

    /// 启动发送 / 接收 / 离线清扫 / 周期统计 四个任务
    pub fn start(self: &Arc<Self>) {
        let sender = self.clone();
        tokio::spawn(async move { sender.send_loop().await });
        let receiver = self.clone();
        tokio::spawn(async move { receiver.recv_loop().await });
        let sweeper = self.clone();
        tokio::spawn(async move { sweeper.sweep_loop().await });
        let stater = self.clone();
        tokio::spawn(async move { stater.stat_loop().await });
    }

    /// 周期打印发现统计（诊断用：一眼看出是否收到对端信标）
    async fn stat_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if self.shutdown.load(Ordering::Relaxed) {
                return;
            }
            let stats = self.stats();
            let peers: Vec<String> = self
                .peers_snapshot()
                .iter()
                .map(|p| format!("{}@{}:{}", p.name, p.ip, p.tcp_port))
                .collect();
            tracing::info!(
                "发现统计 tx={} tx_fail={} rx_total={} rx_valid={} rx_self={} rx_dup={} rx_invalid={} peers=[{}]",
                stats.tx_packets,
                stats.tx_failures,
                stats.rx_total,
                stats.rx_valid,
                stats.rx_self,
                stats.rx_dup,
                stats.rx_invalid,
                peers.join(",")
            );
        }
    }

    fn make_payload(&self, seq: u64) -> Vec<u8> {
        let name = percent_encode(&self.identity.name());
        format!(
            "{}|ver={}|name={}|dev={}|inst={}|seq={}|ts={}|port={}",
            PROTO_NAME,
            PROTO_VER,
            name,
            self.identity.device_id,
            self.instance_id,
            seq,
            now_ms(),
            self.tcp_port
        )
        .into_bytes()
    }

    async fn send_loop(self: Arc<Self>) {
        let mut seq: u64 = 0;
        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                return;
            }
            let interval = {
                let base = self.cfg.interval;
                if self.background.load(Ordering::Relaxed) {
                    base.mul_f32(self.cfg.background_multiplier.max(1.0))
                } else {
                    base
                }
            };
            tokio::time::sleep(interval).await;
            seq += 1;
            let payload = self.make_payload(seq);
            let mut sent = 0u64;
            let mut failed = 0u64;
            let group_addr = SocketAddr::from((self.cfg.group, self.cfg.port));
            // 逐接口组播
            for iface in &self.ifaces {
                match SockRef::from(&*self.sock).set_multicast_if_v4(&iface.ip) {
                    Ok(()) => match self.sock.send_to(&payload, group_addr).await {
                        Ok(_) => sent += 1,
                        Err(e) => {
                            failed += 1;
                            tracing::warn!("组播发送失败 via={} err={}", iface.ip, e);
                        }
                    },
                    Err(e) => {
                        failed += 1;
                        tracing::warn!("设置组播出口失败 via={} err={}", iface.ip, e);
                    }
                }
            }
            // 有限广播
            match self
                .sock
                .send_to(
                    &payload,
                    SocketAddr::from((self.cfg.broadcast, self.cfg.port)),
                )
                .await
            {
                Ok(_) => sent += 1,
                Err(e) => {
                    failed += 1;
                    tracing::warn!("有限广播发送失败 err={}", e);
                }
            }
            // 子网定向广播
            if self.cfg.subnet_broadcast {
                for iface in &self.ifaces {
                    if let Some(b) = iface.broadcast {
                        match self
                            .sock
                            .send_to(&payload, SocketAddr::from((b, self.cfg.port)))
                            .await
                        {
                            Ok(_) => sent += 1,
                            Err(e) => {
                                failed += 1;
                                tracing::warn!("子网广播发送失败 dest={} err={}", b, e);
                            }
                        }
                    }
                }
            }
            let mut stats = self.stats.lock().unwrap();
            stats.tx_packets += sent;
            stats.tx_failures += failed;
        }
    }

    async fn recv_loop(self: Arc<Self>) {
        let mut buf = vec![0u8; 4096];
        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                return;
            }
            let r = tokio::time::timeout(Duration::from_millis(500), self.sock.recv_from(&mut buf))
                .await;
            let (len, addr) = match r {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    tracing::warn!("recv_from 失败: {}", e);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                Err(_) => continue, // 超时：循环检查关闭标志
            };
            let data = &buf[..len];
            {
                let mut stats = self.stats.lock().unwrap();
                stats.rx_total += 1;
            }
            let src_ip = addr.ip().to_string();
            match parse_beacon(data) {
                None => {
                    let mut stats = self.stats.lock().unwrap();
                    stats.rx_invalid += 1;
                    let text = String::from_utf8_lossy(data);
                    let note = if text.starts_with(PROTO_NAME) {
                        "（未知版本或字段缺失）"
                    } else {
                        ""
                    };
                    tracing::debug!("无效报文 from={} bytes={} {}", src_ip, len, note);
                }
                Some(pkt) if pkt.instance_id == self.instance_id => {
                    let mut stats = self.stats.lock().unwrap();
                    stats.rx_self += 1;
                }
                Some(pkt) => {
                    self.handle_beacon(pkt, src_ip).await;
                }
            }
        }
    }

    async fn handle_beacon(&self, pkt: ParsedBeacon, src_ip: String) {
        let mut peers = self.peers.lock().unwrap();
        let mut stats = self.stats.lock().unwrap();
        let prev = peers.get(&pkt.instance_id).cloned();
        match prev {
            None => {
                // 首次发现：同 device_id 的离线旧实例 → 重启替换
                let mut replaced = None;
                let mut remove_key = None;
                for (key, old) in peers.iter() {
                    if old.state_offline_same_device(&pkt) {
                        replaced = Some(old.clone());
                        remove_key = Some(key.clone());
                        break;
                    }
                }
                if let Some(key) = remove_key {
                    peers.remove(&key);
                }
                let entry = PeerEntry {
                    device_id: pkt.device_id.clone(),
                    instance_id: pkt.instance_id.clone(),
                    name: pkt.name.clone(),
                    ip: src_ip.clone(),
                    tcp_port: pkt.port,
                    last_seq: pkt.seq,
                    last_seen: Instant::now(),
                    first_seen_ms: now_ms(),
                    online: true,
                };
                peers.insert(pkt.instance_id.clone(), entry.clone());
                {
                    let mut by_device = self.by_device.lock().unwrap();
                    by_device.insert(
                        pkt.device_id.clone(),
                        PeerAddr {
                            device_id: pkt.device_id.clone(),
                            instance_id: pkt.instance_id.clone(),
                            name: pkt.name.clone(),
                            ip: src_ip,
                            tcp_port: pkt.port,
                            online: true,
                        },
                    );
                }
                stats.rx_valid += 1;
                self.bus.emit(CoreEvent::PeerOnline {
                    device_id: pkt.device_id.clone(),
                    instance_id: pkt.instance_id.clone(),
                    name: pkt.name.clone(),
                    ip: entry.ip.clone(),
                    port: pkt.port,
                });
                if let Some(old) = replaced {
                    tracing::info!(
                        "设备重启替换 dev={} old_inst={} old_name={} new_name={}",
                        pkt.device_id,
                        old.instance_id,
                        old.name,
                        pkt.name
                    );
                }
            }
            Some(mut prev) => {
                if pkt.seq <= prev.last_seq {
                    stats.rx_dup += 1;
                    return;
                }
                stats.rx_valid += 1;
                let name_changed = prev.name != pkt.name;
                let was_offline = !prev.online;
                let old_name = prev.name.clone();
                prev.name = pkt.name.clone();
                prev.ip = src_ip.clone();
                prev.tcp_port = pkt.port;
                prev.last_seq = pkt.seq;
                prev.last_seen = Instant::now();
                prev.online = true;
                peers.insert(pkt.instance_id.clone(), prev.clone());
                {
                    let mut by_device = self.by_device.lock().unwrap();
                    if let Some(addr) = by_device.get_mut(&pkt.device_id) {
                        addr.ip = src_ip.clone();
                        addr.tcp_port = pkt.port;
                        addr.name = pkt.name.clone();
                        addr.online = true;
                    }
                }
                if was_offline {
                    self.bus.emit(CoreEvent::PeerOnline {
                        device_id: pkt.device_id.clone(),
                        instance_id: pkt.instance_id.clone(),
                        name: pkt.name.clone(),
                        ip: src_ip.clone(),
                        port: pkt.port,
                    });
                } else if name_changed {
                    self.bus.emit(CoreEvent::PeerNameChanged {
                        device_id: pkt.device_id.clone(),
                        instance_id: pkt.instance_id.clone(),
                        old_name,
                        new_name: pkt.name,
                    });
                }
            }
        }
    }

    async fn sweep_loop(self: Arc<Self>) {
        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            let timeout = self.cfg.timeout;
            let mut peers = self.peers.lock().unwrap();
            let mut by_device = self.by_device.lock().unwrap();
            let mut offline_events = Vec::new();
            let mut expired = Vec::new();
            for (key, entry) in peers.iter() {
                let elapsed = entry.last_seen.elapsed();
                if entry.online && elapsed >= timeout {
                    offline_events.push((key.clone(), entry.clone()));
                } else if !entry.online && elapsed >= timeout.saturating_mul(4) {
                    expired.push(key.clone());
                }
            }
            for (key, entry) in offline_events {
                if let Some(e) = peers.get_mut(&key) {
                    e.online = false;
                }
                if let Some(addr) = by_device.get_mut(&entry.device_id) {
                    addr.online = false;
                }
                tracing::info!(
                    "设备离线 dev={} inst={} name={}",
                    entry.device_id,
                    entry.instance_id,
                    entry.name
                );
                self.bus.emit(CoreEvent::PeerOffline {
                    device_id: entry.device_id,
                    instance_id: entry.instance_id,
                    name: entry.name,
                });
            }
            for key in expired {
                peers.remove(&key);
            }
        }
    }

    /// 查询某设备当前连接地址
    pub fn peer_addr(&self, device_id: &str) -> Option<PeerAddr> {
        self.by_device.lock().unwrap().get(device_id).cloned()
    }

    /// 当前在线设备快照（供 UI / CLI）
    pub fn peers_snapshot(&self) -> Vec<PeerAddr> {
        let mut v: Vec<PeerAddr> = self.by_device.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn set_background(&self, bg: bool) {
        self.background.store(bg, Ordering::Relaxed);
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    pub fn stats(&self) -> DiscoveryStats {
        *self.stats.lock().unwrap()
    }

    /// 仅测试用：向指定地址直接发送信标（定向发现）
    pub async fn beacon_to(&self, target: SocketAddr) -> Result<()> {
        let payload = self.make_payload(0);
        self.sock.send_to(&payload, target).await?;
        Ok(())
    }

    pub fn iface_count(&self) -> usize {
        self.ifaces.len()
    }

    pub fn random_challenge() -> [u8; 32] {
        let mut c = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut c);
        c
    }
}

impl PeerEntry {
    fn state_offline_same_device(&self, pkt: &ParsedBeacon) -> bool {
        !self.online && self.device_id == pkt.device_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_beacon() {
        let payload = b"LUNOTE1|ver=1|name=%E8%AE%A1%E7%AE%97%E6%9C%BA-A|dev=dev-1|inst=inst-1|seq=5|ts=123|port=45455";
        let p = parse_beacon(payload).unwrap();
        assert_eq!(p.name, "计算机-A");
        assert_eq!(p.device_id, "dev-1");
        assert_eq!(p.seq, 5);
        assert_eq!(p.port, 45455);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_beacon(b"hello world").is_none());
        assert!(
            parse_beacon(b"LUNOTE1|ver=99|name=x|dev=d|inst=i|seq=1|ts=1|port=45454").is_none()
        );
        assert!(parse_beacon(b"LUNOTE1|ver=1|name=|dev=d|inst=i|seq=1|ts=1|port=45454").is_none());
        assert!(parse_beacon(b"LUNOTE1|ver=1|name=x|dev=d|inst=i|seq=1|ts=1|port=0").is_none());
        assert!(
            parse_beacon(b"LUNOTE1|ver=1|name=x|dev=d|inst=i|seq=abc|ts=1|port=45454").is_none()
        );
    }

    #[test]
    fn percent_roundtrip() {
        let s = "特殊#1 设备&测试 @#$%^&*()";
        assert_eq!(percent_decode(&percent_encode(s)), s);
    }
}
