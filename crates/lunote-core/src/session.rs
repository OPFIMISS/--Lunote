//! TLS 1.3 会话层：双向认证通道、应用层签名挑战、帧分发、心跳。
//!
//! 安全模型（如实说明，详见 docs/安全模型.md）：
//! - TLS 1.3（仅 1.3，禁 1.2），通道加密由 rustls(ring) 提供；
//! - 双方在握手时各出示自签名证书；rustls 层对证书做“结构校验”（本产品自管信任，
//!   信任锚不是 CA 层级），身份绑定在应用层完成；
//! - 应用层：服务端先发 32 字节随机 challenge；客户端回 `Hello`（证书 + 签名）；
//!   服务端回 `HelloAck`（同样签名）。签名覆盖 challenge||device_id||name||instance_id，
//!   每次连接新鲜随机，防重放；
//! - 指纹 = SHA-256(证书 DER)。与 TOFU/信任库比对：New（首次见面，记录指纹）、
//!   Same（正常）、Changed（身份变化 → 警告 + 降级为不可信）。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use ed25519_dalek::Signature;
use rand::RngCore;
use rustls_pki_types::{CertificateDer, UnixTime};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{lookup_host, TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::discovery::Discovery;
use crate::events::{CoreEvent, Direction, EventBus, MsgKind};
use crate::identity::{extract_ed25519_pubkey, sha256_hex, DeviceIdentity};
use crate::messages::{read_frame, signed_data, write_frame, Control, Frame, CHALLENGE_LEN};
use crate::store::Store;
use crate::transfer::TransferManager;
use crate::trust::TrustStore;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const IDLE_CLOSE_AFTER: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_TEXT_LEN: usize = 64 * 1024;
const MAX_LINK_LEN: usize = 8 * 1024;
const CHANNEL_CAPACITY: usize = 128;
const QUEUE_SEND_TIMEOUT: Duration = Duration::from_secs(5);

enum Outbound {
    Frame(Frame),
    Close(String),
}

#[derive(Clone)]
struct SessionKey {
    generation: Arc<AtomicU64>,
    tx: mpsc::Sender<Outbound>,
    close_tx: tokio::sync::watch::Sender<bool>,
}

pub struct SessionManager {
    identity: Arc<DeviceIdentity>,
    trust: Arc<Mutex<TrustStore>>,
    store: Arc<Store>,
    bus: EventBus,
    discovery: Arc<Discovery>,
    pub transfers: Arc<TransferManager>,
    acceptor: TlsAcceptor,
    connector: TlsConnector,
    listener: Mutex<Option<Arc<TcpListener>>>,
    sessions: Mutex<HashMap<String, SessionKey>>,
    peer_names: Mutex<HashMap<String, String>>,
    connection_gen: AtomicU64,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    listener_closed_tx: std::sync::mpsc::Sender<()>,
    listener_closed_rx: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// “同名同 IP 新设备自动信任”开关（默认开，由 Runtime 持久化设置）
    pub auto_trust: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl SessionManager {
    pub fn new(
        identity: Arc<DeviceIdentity>,
        trust: Arc<Mutex<TrustStore>>,
        store: Arc<Store>,
        bus: EventBus,
        discovery: Arc<Discovery>,
        transfers: Arc<TransferManager>,
        auto_trust: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Arc<Self>> {
        let provider = rustls::crypto::ring::default_provider();
        let cert_der = CertificateDer::from(identity.cert_der.clone());
        let key_der = identity.tls_key_der();

        let server_cfg = rustls::ServerConfig::builder_with_provider(Arc::new(provider.clone()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .context("TLS 1.3 协议版本不可用")?
            .with_client_cert_verifier(Arc::new(AcceptAnyClientCertVerifier))
            .with_single_cert(vec![cert_der.clone()], key_der.clone_key())
            .context("服务器证书配置失败")?;

        let client_cfg = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .context("TLS 1.3 协议版本不可用")?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCertVerifier))
            .with_client_auth_cert(vec![cert_der], key_der)
            .context("客户端证书配置失败")?;

        let (closed_tx, closed_rx) = std::sync::mpsc::channel::<()>();
        Ok(Arc::new(Self {
            identity,
            trust,
            store,
            bus,
            discovery,
            transfers,
            acceptor: TlsAcceptor::from(Arc::new(server_cfg)),
            connector: TlsConnector::from(Arc::new(client_cfg)),
            listener: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            peer_names: Mutex::new(HashMap::new()),
            connection_gen: AtomicU64::new(0),
            shutdown: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            shutdown_tx: tokio::sync::watch::channel(false).0,
            listener_closed_tx: closed_tx,
            listener_closed_rx: Mutex::new(Some(closed_rx)),
            auto_trust,
        }))
    }

    /// 启动 TCP 监听（固定端口，与发现信标广播的 port 一致）
    pub async fn start(self: &Arc<Self>, tcp_port: u16) -> Result<u16> {
        // SO_REUSEADDR：Windows 上进程快速重启时允许立即重新绑定
        let sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .context("创建监听 socket 失败")?;
        sock.set_reuse_address(true)?;
        sock.bind(&std::net::SocketAddr::from(([0, 0, 0, 0], tcp_port)).into())
            .with_context(|| format!("绑定 TCP 端口 {} 失败", tcp_port))?;
        sock.listen(128)?;
        sock.set_nonblocking(true)?;
        let std_listener: std::net::TcpListener = sock.into();
        let listener = Arc::new(TcpListener::from_std(std_listener)?);
        let local = listener.local_addr()?;
        *self.listener.lock().unwrap() = Some(listener.clone());
        let this = self.clone();
        tokio::spawn(async move { this.accept_loop(listener).await });
        tracing::info!("会话监听启动 tcp_port={}", local.port());
        Ok(local.port())
    }

    async fn accept_loop(self: &Arc<Self>, listener: Arc<TcpListener>) {
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
                accepted = listener.accept() => {
                    match accepted {
                        Ok((tcp, addr)) => {
                            let this = self.clone();
                            tokio::spawn(async move {
                                if let Err(e) = this.server_conn(tcp, addr).await {
                                    tracing::debug!("连接结束 from={} err={}", addr, e);
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!("accept 失败: {}", e);
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        }
                    }
                }
            }
        }
        let _ = self.listener_closed_tx.send(());
    }

    async fn server_conn(self: Arc<Self>, tcp: TcpStream, addr: SocketAddr) -> Result<()> {
        let tls = tokio::time::timeout(Duration::from_secs(15), self.acceptor.accept(tcp))
            .await
            .context("TLS 握手超时（服务器侧）")?
            .context("TLS 握手失败（服务器侧）")?;
        let mut stream = tls;
        // 1) 发送随机 challenge（原始字节）
        let challenge = random_bytes();
        stream.write_all(&challenge).await?;
        let challenge_b64 = b64(&challenge);
        // 2) 读取 Hello（带超时保护）
        let frame = tokio::time::timeout(Duration::from_secs(15), read_frame(&mut stream))
            .await
            .context("等待 Hello 超时")??;
        let Control::Hello {
            device_id,
            name,
            instance_id,
            challenge: client_challenge_b64,
            cert_b64,
            sig_b64,
            ts_ms: _,
        } = frame
            .as_control()
            .ok_or_else(|| anyhow!("握手首帧不是 Hello"))?
        else {
            bail!("握手首帧类型错误");
        };
        validate_identity_fields(&device_id, &name, &instance_id)?;

        // 3) 验证证书 + 签名
        let cert_der = b64_decode(&cert_b64)?;
        let fingerprint = verify_cert_sig(
            &cert_der,
            &sig_b64,
            &challenge_b64,
            &device_id,
            &name,
            &instance_id,
        )?;

        // 4) TOFU / 信任核对
        let (check, trusted, is_new) = {
            let mut trust = self.trust.lock().unwrap();
            let check = trust.check_identity(&device_id, &fingerprint, &name);
            let trusted = match check {
                crate::trust::IdentityCheck::New => false,
                crate::trust::IdentityCheck::Same => trust.is_trusted(&device_id),
                crate::trust::IdentityCheck::Changed => false,
            };
            (check, trusted, check == crate::trust::IdentityCheck::New)
        };
        // 自动信任：全新设备且开启时，若与已信任设备“同名同 IP” → 自动确认
        let mut auto_trusted = false;
        if is_new && self.auto_trust.load(Ordering::Relaxed) {
            let ip = addr.ip().to_string();
            let mut trust = self.trust.lock().unwrap();
            if trust.auto_trust_match(&name, &ip) {
                if let Err(e) = trust.set_trusted(&device_id, true) {
                    tracing::warn!("自动信任写入失败 dev={} err={}", device_id, e);
                } else {
                    auto_trusted = true;
                    tracing::info!(
                        "自动信任（同名同 IP）dev={} name={} ip={}",
                        device_id,
                        name,
                        ip
                    );
                }
            }
        }
        // 签名握手验证通过：记录对端真实连接 IP（仅此处更新——发现信标不可信，
        // 若在发现层更新 last_ip，伪造信标可覆写并配合自动信任提升攻击面）
        self.trust
            .lock()
            .unwrap()
            .update_last_ip(&device_id, &addr.ip().to_string());
        if check == crate::trust::IdentityCheck::Changed {
            let old = self
                .trust
                .lock()
                .unwrap()
                .get(&device_id)
                .map(|r| r.fingerprint.clone())
                .unwrap_or_default();
            tracing::warn!(
                "身份指纹变化 dev={} old={} new={}（降级为不可信）",
                device_id,
                old,
                fingerprint
            );
            self.bus.emit(CoreEvent::IdentityChanged {
                device_id: device_id.clone(),
                name: name.clone(),
                old_fingerprint: old,
                new_fingerprint: fingerprint,
            });
        }

        // 5) 回 HelloAck（签名客户端 challenge）
        let ack_sig = self.identity.sign(&signed_data(
            &client_challenge_b64,
            &self.identity.device_id,
            &self.identity.name(),
            &self.identity.instance_id,
        ));
        let ack = Control::HelloAck {
            device_id: self.identity.device_id.clone(),
            name: self.identity.name(),
            instance_id: self.identity.instance_id.clone(),
            cert_b64: b64(&self.identity.cert_der),
            sig_b64: b64(ack_sig.to_bytes().as_ref()),
            ts_ms: now_ms(),
        };
        write_frame(&mut stream, 0, &serde_json::to_vec(&ack)?).await?;

        self.bus.emit(CoreEvent::PeerConnected {
            device_id: device_id.clone(),
            name: name.clone(),
            trusted: trusted || auto_trusted,
            is_new_device: is_new,
            auto_trusted,
        });
        tracing::info!(
            "对端接入 dev={} name={} trusted={} new={} auto_trusted={}",
            device_id,
            name,
            trusted || auto_trusted,
            is_new,
            auto_trusted
        );
        self.run_connection(
            stream,
            device_id,
            name,
            trusted || auto_trusted,
            "接入".into(),
            None,
        )
        .await
    }

    /// 主动连接（发现层已给出地址；离线/失败时返回错误）
    pub async fn connect_to(self: &Arc<Self>, device_id: &str) -> Result<()> {
        if self.is_connected(device_id) {
            return Ok(());
        }
        let peer = self
            .discovery
            .peer_addr(device_id)
            .ok_or_else(|| anyhow!("设备 {} 不在线（无发现记录）", device_id))?;
        if !peer.online {
            bail!("设备 {} 当前离线", peer.name);
        }
        let addr: SocketAddr = format!("{}:{}", peer.ip, peer.tcp_port).parse()?;
        self.connect_addr(addr, Some(device_id)).await.map(|_| ())
    }

    /// 通过用户输入的地址主动连接。身份仍由证书签名握手确认，不降低 TOFU 校验。
    pub async fn connect_address(
        self: &Arc<Self>,
        host: &str,
        port: u16,
    ) -> Result<(String, String)> {
        let host = host.trim().trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() {
            bail!("请输入设备 IP 或主机名");
        }
        let mut addresses = lookup_host((host, port))
            .await
            .with_context(|| format!("无法解析地址 {}:{}", host, port))?;
        let addr = addresses
            .next()
            .ok_or_else(|| anyhow!("地址 {}:{} 没有可用结果", host, port))?;
        self.connect_addr(addr, None).await
    }

    async fn connect_addr(
        self: &Arc<Self>,
        addr: SocketAddr,
        expected_device_id: Option<&str>,
    ) -> Result<(String, String)> {
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
            .await
            .context("连接超时")?
            .with_context(|| format!("TCP 连接 {} 失败", addr))?;
        let server_name = rustls_pki_types::ServerName::try_from("lunote.local")
            .map_err(|_| anyhow!("服务器名非法"))?;
        let tls = tokio::time::timeout(
            Duration::from_secs(15),
            self.connector.connect(server_name, tcp),
        )
        .await
        .context("TLS 握手超时（客户端侧）")?
        .context("TLS 握手失败（客户端侧）")?;
        let mut stream = tls;

        // 身份握手（带超时保护）
        let (peer_id, peer_name, _peer_inst, _peer_cert_der, peer_fingerprint, _my_challenge_b64) =
            tokio::time::timeout(Duration::from_secs(15), async {
                // 1) 读服务端 challenge
                let mut challenge = [0u8; CHALLENGE_LEN];
                stream.read_exact(&mut challenge).await?;
                let challenge_b64 = b64(&challenge);

                // 2) 发 Hello
                let my_challenge = random_bytes();
                let sig = self.identity.sign(&signed_data(
                    &challenge_b64,
                    &self.identity.device_id,
                    &self.identity.name(),
                    &self.identity.instance_id,
                ));
                let hello = Control::Hello {
                    device_id: self.identity.device_id.clone(),
                    name: self.identity.name(),
                    instance_id: self.identity.instance_id.clone(),
                    challenge: b64(&my_challenge),
                    cert_b64: b64(&self.identity.cert_der),
                    sig_b64: b64(sig.to_bytes().as_ref()),
                    ts_ms: now_ms(),
                };
                write_frame(&mut stream, 0, &serde_json::to_vec(&hello)?).await?;

                // 3) 读 HelloAck 并验证
                let frame = read_frame(&mut stream).await?;
                let Control::HelloAck {
                    device_id: peer_id,
                    name: peer_name,
                    instance_id: peer_inst,
                    cert_b64: peer_cert_b64,
                    sig_b64: peer_sig_b64,
                    ts_ms: _,
                } = frame
                    .as_control()
                    .ok_or_else(|| anyhow!("握手应答不是 HelloAck"))?
                else {
                    bail!("握手应答类型错误");
                };
                let peer_cert_der = b64_decode(&peer_cert_b64)?;
                let peer_fingerprint = verify_cert_sig(
                    &peer_cert_der,
                    &peer_sig_b64,
                    &b64(&my_challenge),
                    &peer_id,
                    &peer_name,
                    &peer_inst,
                )?;
                Ok::<_, anyhow::Error>((
                    peer_id,
                    peer_name,
                    peer_inst,
                    peer_cert_der,
                    peer_fingerprint,
                    b64(&my_challenge),
                ))
            })
            .await
            .context("身份握手超时（客户端侧）")??;
        if let Some(expected) = expected_device_id {
            if peer_id != expected {
                bail!(
                    "发现地址指向的设备与握手身份不一致（{} vs {}），可能被冒充",
                    peer_id,
                    expected
                );
            }
        }
        let (check, trusted, is_new) = {
            let mut trust = self.trust.lock().unwrap();
            let check = trust.check_identity(&peer_id, &peer_fingerprint, &peer_name);
            let trusted = match check {
                crate::trust::IdentityCheck::New => false,
                crate::trust::IdentityCheck::Same => trust.is_trusted(&peer_id),
                crate::trust::IdentityCheck::Changed => false,
            };
            (check, trusted, check == crate::trust::IdentityCheck::New)
        };
        // 自动信任：全新设备且开启时，若与已信任设备“同名同 IP” → 自动确认
        let mut auto_trusted = false;
        if is_new && self.auto_trust.load(Ordering::Relaxed) {
            let ip = addr.ip().to_string();
            let mut trust = self.trust.lock().unwrap();
            if trust.auto_trust_match(&peer_name, &ip) {
                if let Err(e) = trust.set_trusted(&peer_id, true) {
                    tracing::warn!("自动信任写入失败 dev={} err={}", peer_id, e);
                } else {
                    auto_trusted = true;
                    tracing::info!(
                        "自动信任（同名同 IP）dev={} name={} ip={}",
                        peer_id,
                        peer_name,
                        ip
                    );
                }
            }
        }
        // 签名握手验证通过：记录对端真实连接 IP（仅此处更新，发现信标不可信）
        self.trust
            .lock()
            .unwrap()
            .update_last_ip(&peer_id, &addr.ip().to_string());
        if check == crate::trust::IdentityCheck::Changed {
            self.bus.emit(CoreEvent::IdentityChanged {
                device_id: peer_id.clone(),
                name: peer_name.clone(),
                old_fingerprint: self
                    .trust
                    .lock()
                    .unwrap()
                    .get(&peer_id)
                    .map(|r| r.fingerprint.clone())
                    .unwrap_or_default(),
                new_fingerprint: peer_fingerprint,
            });
        }
        self.bus.emit(CoreEvent::PeerConnected {
            device_id: peer_id.clone(),
            name: peer_name.clone(),
            trusted: trusted || auto_trusted,
            is_new_device: is_new,
            auto_trusted,
        });
        tracing::info!(
            "已连接 dev={} name={} trusted={} new={} auto_trusted={}",
            peer_id,
            peer_name,
            trusted || auto_trusted,
            is_new,
            auto_trusted
        );
        // 等待 run_connection 完成会话注册后再返回，避免“刚连接成功立刻发送”竞态。
        let connected_id = peer_id.clone();
        let connected_name = peer_name.clone();
        let this = self.clone();
        let (ready_tx, ready_rx) = oneshot::channel();
        tokio::spawn(async move {
            let _ = this
                .run_connection(
                    stream,
                    peer_id,
                    peer_name,
                    trusted || auto_trusted,
                    "主动连接".into(),
                    Some(ready_tx),
                )
                .await;
        });
        tokio::time::timeout(CONNECT_TIMEOUT, ready_rx)
            .await
            .context("等待会话注册超时")?
            .context("会话在注册前结束")?;
        Ok((connected_id, connected_name))
    }

    /// 连接主循环（收发帧、心跳、分发）
    async fn run_connection<S>(
        &self,
        stream: S,
        device_id: String,
        name: String,
        trusted: bool,
        via: String,
        ready: Option<oneshot::Sender<()>>,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (read_half, write_half) = tokio::io::split(stream);
        let (tx, mut rx) = mpsc::channel::<Outbound>(CHANNEL_CAPACITY);
        let (close_tx, mut close_rx) = tokio::sync::watch::channel(false);
        // 入站帧通道：reader 任务独占读半流，主循环消费（杜绝 select 取消半途读取丢字节）
        let (frame_tx, mut frame_rx) = mpsc::channel::<Result<Frame, String>>(64);
        let generation = Arc::new(AtomicU64::new(
            self.connection_gen.fetch_add(1, Ordering::SeqCst) + 1,
        ));
        {
            let mut sessions = self.sessions.lock().unwrap();
            // 同设备新连接替换旧连接
            if let Some(old) = sessions.insert(
                device_id.clone(),
                SessionKey {
                    generation: generation.clone(),
                    tx: tx.clone(),
                    close_tx: close_tx.clone(),
                },
            ) {
                let _ = old.close_tx.send(true);
                tracing::info!("替换旧连接 dev={}", device_id);
            }
        }
        self.peer_names
            .lock()
            .unwrap()
            .insert(device_id.clone(), name.clone());
        if let Some(ready) = ready {
            let _ = ready.send(());
        }

        // 读任务：独占读半流（连接关闭时经 close 信号退出；半途取消只发生在连接废弃时，无副作用）
        let reader_dev_id = device_id.clone();
        let mut reader_close = close_rx.clone();
        let reader = tokio::spawn(async move {
            let mut read_half = read_half;
            loop {
                tokio::select! {
                    r = read_frame(&mut read_half) => {
                        match r {
                            Ok(frame) => {
                                tracing::debug!("[conn {}] 收到帧 kind={} len={}", reader_dev_id, frame.kind, frame.payload.len());
                                if frame_tx.send(Ok(frame)).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                let _ = frame_tx.send(Err(e.to_string())).await;
                                break;
                            }
                        }
                    }
                    _ = reader_close.changed() => {
                        if *reader_close.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        // 写任务：独占写半流，杜绝 select 取消半途写入导致的字节流错乱
        let writer_dev_id = device_id.clone();
        let writer = tokio::spawn(async move {
            let mut write_half = write_half;
            while let Some(msg) = rx.recv().await {
                match msg {
                    Outbound::Frame(frame) => {
                        tracing::debug!(
                            "[conn {}] 发送帧 kind={} len={}",
                            writer_dev_id,
                            frame.kind,
                            frame.payload.len()
                        );
                        if let Err(e) =
                            write_frame(&mut write_half, frame.kind, &frame.payload).await
                        {
                            tracing::debug!("写帧失败 dev={}: {}", writer_dev_id, e);
                            break;
                        }
                    }
                    Outbound::Close(reason) => {
                        tracing::debug!("写任务结束 dev={} reason={}", writer_dev_id, reason);
                        break;
                    }
                }
            }
        });

        let mut last_rx = Instant::now();
        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut close_reason: Option<String> = None;

        loop {
            let is_current = self
                .sessions
                .lock()
                .unwrap()
                .get(&device_id)
                .map(|current| Arc::ptr_eq(&current.generation, &generation))
                .unwrap_or(false);
            if !is_current {
                close_reason = Some("连接已被新连接替换".into());
                break;
            }
            tokio::select! {
                frame = frame_rx.recv() => {
                    match frame {
                        Some(Ok(frame)) => {
                            last_rx = Instant::now();
                            if let Err(e) = self.dispatch_frame(&device_id, &name, trusted, frame).await {
                                close_reason = Some(format!("处理帧失败: {}", e));
                                break;
                            }
                        }
                        Some(Err(e)) => {
                            close_reason = Some(format!("读帧失败: {}", e));
                            break;
                        }
                        None => {
                            close_reason = Some("读任务退出".into());
                            break;
                        }
                    }
                }
                _ = close_rx.changed() => {
                    if *close_rx.borrow() {
                        close_reason = Some("连接已关闭（应用关闭或新连接替换）".into());
                        break;
                    }
                }
                _ = heartbeat.tick() => {
                    // 心跳也走通道，与业务帧串行写出
                    let ping = Control::Ping { ts_ms: now_ms() };
                    let frame = Frame::json(&ping).map_err(|e| anyhow!(e))?;
                    let key = self.sessions.lock().unwrap().get(&device_id).cloned();
                    if let Some(key) = key {
                        match tokio::time::timeout(
                            QUEUE_SEND_TIMEOUT,
                            key.tx.send(Outbound::Frame(frame)),
                        )
                        .await
                        {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => {
                                close_reason = Some("心跳通道已关闭".into());
                                break;
                            }
                            Err(_) => {
                                close_reason = Some("发送队列阻塞超时".into());
                                break;
                            }
                        }
                    }
                    if last_rx.elapsed() > IDLE_CLOSE_AFTER {
                        close_reason = Some("空闲超时".into());
                        break;
                    }
                }
            }
        }
        // 关闭通道 → 读/写任务自行退出（不 await，避免写阻塞导致清理挂起）
        drop(tx);
        drop(frame_rx);
        let _ = close_tx.send(true);
        drop(reader);
        drop(writer);

        // 清理
        {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(cur) = sessions.get(&device_id) {
                if Arc::ptr_eq(&cur.generation, &generation) {
                    sessions.remove(&device_id);
                }
            }
        }
        let reason = close_reason.unwrap_or_else(|| "未知原因".into());
        tracing::info!("会话结束 dev={} reason={} via={}", device_id, reason, via);
        self.bus.emit(CoreEvent::PeerDisconnected {
            device_id: device_id.clone(),
            reason: reason.clone(),
        });
        // 通知传输层该对端断开
        self.transfers
            .on_peer_disconnected(&device_id, &reason)
            .await;
        Ok(())
    }

    async fn dispatch_frame(
        &self,
        device_id: &str,
        _name: &str,
        trusted: bool,
        frame: Frame,
    ) -> Result<()> {
        match frame.kind {
            crate::messages::FRAME_CHUNK => {
                self.transfers.on_chunk(device_id, frame.payload).await;
                Ok(())
            }
            crate::messages::FRAME_JSON => {
                let Some(control) = frame.as_control() else {
                    tracing::warn!("来自 {} 的控制帧解析失败", device_id);
                    return Ok(());
                };
                match control {
                    Control::Text { id, text, ts_ms } => {
                        if text.len() > MAX_TEXT_LEN {
                            bail!("文本超长（{} 字节）", text.len());
                        }
                        self.store.append_message(
                            device_id,
                            &id,
                            Direction::Incoming,
                            MsgKind::Text,
                            &text,
                            None,
                            ts_ms,
                        )?;
                        self.bus.emit(CoreEvent::MessageReceived {
                            device_id: device_id.to_string(),
                            message_id: id,
                            kind: MsgKind::Text,
                            text,
                            ts_ms,
                            from_untrusted: !trusted,
                        });
                        self.bus.emit(CoreEvent::RecordsChanged);
                        Ok(())
                    }
                    Control::Link {
                        id,
                        url,
                        title,
                        ts_ms,
                    } => {
                        if url.len() > MAX_LINK_LEN {
                            bail!("链接超长（{} 字节）", url.len());
                        }
                        // title 限制 256 字节，防单条记录膨胀（UTF-8 安全截断）
                        let title = title.map(|mut t| {
                            if t.len() > 256 {
                                let mut end = 256;
                                while !t.is_char_boundary(end) {
                                    end -= 1;
                                }
                                t.truncate(end);
                            }
                            t
                        });
                        let text = match &title {
                            Some(t) => format!("{} {}", t, url),
                            None => url.clone(),
                        };
                        self.store.append_message(
                            device_id,
                            &id,
                            Direction::Incoming,
                            MsgKind::Link,
                            &text,
                            Some(&url),
                            ts_ms,
                        )?;
                        self.bus.emit(CoreEvent::MessageReceived {
                            device_id: device_id.to_string(),
                            message_id: id,
                            kind: MsgKind::Link,
                            text,
                            ts_ms,
                            from_untrusted: !trusted,
                        });
                        self.bus.emit(CoreEvent::RecordsChanged);
                        Ok(())
                    }
                    Control::Ping { ts_ms } => {
                        let pong = Control::Pong { ts_ms };
                        let frame = Frame::json(&pong)?;
                        self.push_frame(device_id, frame).await?;
                        Ok(())
                    }
                    Control::Pong { .. } => Ok(()),
                    Control::Bye { reason } => {
                        tracing::info!("对端告别 dev={} reason={}", device_id, reason);
                        Ok(())
                    }
                    other => {
                        self.transfers.on_control(device_id, other).await;
                        Ok(())
                    }
                }
            }
            other => {
                tracing::warn!("未知帧类型 {} 来自 {}", other, device_id);
                Ok(())
            }
        }
    }

    /// 发送文字（自动建连；返回消息 ID）
    pub async fn send_text(self: &Arc<Self>, device_id: &str, text: &str) -> Result<String> {
        if text.is_empty() || text.len() > MAX_TEXT_LEN {
            bail!("文本长度非法（1~{} 字节）", MAX_TEXT_LEN);
        }
        self.ensure_session(device_id).await?;
        let id = crate::messages::new_id();
        let ts = now_ms();
        let control = Control::Text {
            id: id.clone(),
            text: text.to_string(),
            ts_ms: ts,
        };
        self.send_control(device_id, control).await?;
        self.store.append_message(
            device_id,
            &id,
            Direction::Outgoing,
            MsgKind::Text,
            text,
            None,
            ts,
        )?;
        self.bus.emit(CoreEvent::MessageSent {
            device_id: device_id.to_string(),
            message_id: id.clone(),
            kind: MsgKind::Text,
            text: text.to_string(),
            ts_ms: ts,
        });
        self.bus.emit(CoreEvent::RecordsChanged);
        Ok(id)
    }

    /// 发送链接
    pub async fn send_link(
        self: &Arc<Self>,
        device_id: &str,
        url: &str,
        title: Option<&str>,
    ) -> Result<String> {
        if url.is_empty() || url.len() > MAX_LINK_LEN {
            bail!("链接长度非法");
        }
        self.ensure_session(device_id).await?;
        let id = crate::messages::new_id();
        let ts = now_ms();
        let control = Control::Link {
            id: id.clone(),
            url: url.to_string(),
            title: title.map(|s| s.to_string()),
            ts_ms: ts,
        };
        self.send_control(device_id, control).await?;
        let text = match title {
            Some(t) => format!("{} {}", t, url),
            None => url.to_string(),
        };
        self.store.append_message(
            device_id,
            &id,
            Direction::Outgoing,
            MsgKind::Link,
            &text,
            Some(url),
            ts,
        )?;
        self.bus.emit(CoreEvent::MessageSent {
            device_id: device_id.to_string(),
            message_id: id.clone(),
            kind: MsgKind::Link,
            text,
            ts_ms: ts,
        });
        self.bus.emit(CoreEvent::RecordsChanged);
        Ok(id)
    }

    /// 向会话通道推送控制帧（无会话则报错）
    pub async fn send_control(&self, device_id: &str, control: Control) -> Result<()> {
        let frame = Frame::json(&control)?;
        self.push_frame(device_id, frame).await
    }

    pub async fn send_chunk(&self, device_id: &str, payload: Vec<u8>) -> Result<()> {
        let frame = Frame::chunk(payload);
        self.push_frame(device_id, frame).await
    }

    pub async fn push_frame(&self, device_id: &str, frame: Frame) -> Result<()> {
        let key = self
            .sessions
            .lock()
            .unwrap()
            .get(device_id)
            .cloned()
            .ok_or_else(|| anyhow!("设备 {} 无活动会话", device_id))?;
        match tokio::time::timeout(QUEUE_SEND_TIMEOUT, key.tx.send(Outbound::Frame(frame))).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(anyhow!("设备 {} 会话已关闭", device_id)),
            Err(_) => Err(anyhow!("设备 {} 发送队列阻塞超时", device_id)),
        }
    }

    pub fn is_connected(&self, device_id: &str) -> bool {
        self.sessions.lock().unwrap().contains_key(device_id)
    }

    pub fn connected_devices(&self) -> Vec<String> {
        self.sessions.lock().unwrap().keys().cloned().collect()
    }

    pub fn peer_name(&self, device_id: &str) -> Option<String> {
        self.peer_names.lock().unwrap().get(device_id).cloned()
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.shutdown_tx.send(true);
        // 等待监听任务退出并释放端口（最多 2 秒）
        if let Some(rx) = self.listener_closed_rx.lock().unwrap().take() {
            let _ = rx.recv_timeout(Duration::from_secs(2));
        }
        // 关闭现有连接
        let keys: Vec<SessionKey> = self.sessions.lock().unwrap().values().cloned().collect();
        for k in keys {
            let _ = k.close_tx.send(true);
            let _ = k.tx.try_send(Outbound::Close("应用关闭".into()));
        }
    }

    async fn ensure_session(self: &Arc<Self>, device_id: &str) -> Result<()> {
        if self.is_connected(device_id) {
            return Ok(());
        }
        self.connect_to(device_id).await
    }
}

// ---------- 工具函数 ----------

fn random_bytes() -> [u8; CHALLENGE_LEN] {
    let mut b = [0u8; CHALLENGE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b
}

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn b64_decode(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|_| anyhow!("base64 解码失败"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn validate_identity_fields(device_id: &str, name: &str, instance_id: &str) -> Result<()> {
    if device_id.is_empty() || device_id.len() > 128 {
        bail!("device_id 非法");
    }
    if name.is_empty() || name.len() > 256 {
        bail!("设备名非法");
    }
    if instance_id.is_empty() || instance_id.len() > 128 {
        bail!("instance_id 非法");
    }
    Ok(())
}

/// 校验证书 + 签名，返回证书指纹
fn verify_cert_sig(
    cert_der: &[u8],
    sig_b64: &str,
    challenge_b64: &str,
    device_id: &str,
    name: &str,
    instance_id: &str,
) -> Result<String> {
    let pubkey = extract_ed25519_pubkey(cert_der)
        .ok_or_else(|| anyhow!("证书不是有效的 Ed25519 自签名证书"))?;
    let sig_bytes = b64_decode(sig_b64)?;
    let sig: [u8; 64] = sig_bytes.try_into().map_err(|_| anyhow!("签名长度非法"))?;
    let signature = Signature::from_bytes(&sig);
    let data = signed_data(challenge_b64, device_id, name, instance_id);
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&pubkey)?;
    vk.verify_strict(&data, &signature)
        .map_err(|_| anyhow!("签名验证失败（身份声明与密钥不符）"))?;
    Ok(sha256_hex(cert_der))
}

// ---------- TLS 验证器 ----------
// 说明：结构校验 + 应用层绑定。见模块文档的安全模型。

#[derive(Debug)]
struct AcceptAnyClientCertVerifier;

impl rustls::server::danger::ClientCertVerifier for AcceptAnyClientCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![rustls::SignatureScheme::ED25519]
    }
}

#[derive(Debug)]
struct AcceptAnyServerCertVerifier;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls_pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![rustls::SignatureScheme::ED25519]
    }
}
