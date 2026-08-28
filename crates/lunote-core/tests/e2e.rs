//! 端到端集成测试：真实 socket（本机多实例，UDP 组播回环 + TCP 回环）。
//!
//! 状态定位：这些测试属于“仅在本机验证”（同一台机器上的多实例），
//! 不构成跨设备验证。发现端口统一 45454（SO_REUSEADDR 共享），
//! 每测试使用不同 TCP 端口避免冲突。

use std::path::Path;
use std::time::Duration;

/// 调试标记：直接写文件（绕过 cargo 输出缓冲，便于实时观察卡点）
#[allow(dead_code)]
fn mark(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(r"D:\Lunote 2\.toolchains\e2e-marks.log")
    {
        let _ = writeln!(f, "{} {}", std::process::id(), msg);
    }
}

/// 测试用 tracing 文件日志（tracing-subscriber 为 dev-dependency）
#[allow(dead_code)]
fn init_tracing() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let f = std::fs::File::create(r"D:\Lunote 2\.toolchains\e2e-trace.log").unwrap();
        let _ = tracing_subscriber::fmt()
            .with_writer(f)
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .try_init();
    });
}

use lunote_core::events::{CoreEvent, Direction, TransferState};
use lunote_core::{Runtime, RuntimeConfig};
use tokio::sync::broadcast;

fn cfg(dir: &Path, name: &str, tcp_port: u16) -> RuntimeConfig {
    RuntimeConfig {
        data_dir: dir.to_path_buf(),
        name: name.to_string(),
        discovery_port: 45454,
        tcp_port,
        discovery_interval: Duration::from_millis(250),
        offline_timeout: Duration::from_secs(1),
        downloads_dir: None,
        background: false,
    }
}

/// 在超时内等待匹配事件，返回捕获值
async fn wait_for<T>(
    rx: &mut broadcast::Receiver<CoreEvent>,
    pat: impl Fn(&CoreEvent) -> Option<T>,
    secs: u64,
) -> Option<T> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Ok(ev)) => {
                if let Some(t) = pat(&ev) {
                    return Some(t);
                }
            }
            // 落后于广播队列：跳到最新，继续等待（不能当作失败）
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => return None,
            Err(_) => {}
        }
    }
    None
}

/// 等待双方在发现快照中互相可见（轮询快照，避免广播事件订阅时机问题）
async fn wait_discovered(a: &Runtime, b: &Runtime, secs: u64) {
    let b_id = b.identity.device_id.clone();
    let a_id = a.identity.device_id.clone();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let a_found = a.peers().iter().any(|p| p.device_id == b_id && p.online);
        let b_found = b.peers().iter().any(|p| p.device_id == a_id && p.online);
        if a_found && b_found {
            return;
        }
        if tokio::time::Instant::now() > deadline {
            panic!(
                "发现超时：A见B={} B见A={}（A peers={:?} B peers={:?}）",
                a_found,
                b_found,
                a.peers(),
                b.peers()
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn trust_both(a: &Runtime, b: &Runtime) {
    let a_id = a.identity.device_id.clone();
    let b_id = b.identity.device_id.clone();
    let a_name = a.identity.name();
    let b_name = b.identity.name();
    a.trust_device(&b_id, &b_name, true).unwrap();
    b.trust_device(&a_id, &a_name, true).unwrap();
}

fn rand_bytes(n: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut v = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut v);
    v
}

fn sha256_file(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    // Windows Defender 等安全软件可能瞬时锁定刚落盘的文件：短暂重试
    let mut f = None;
    for attempt in 0..20 {
        match std::fs::File::open(path) {
            Ok(fh) => {
                f = Some(fh);
                break;
            }
            Err(e) if attempt == 19 => {
                panic!("sha256_file 打开失败: {} ({})", path.display(), e);
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(150)),
        }
    }
    let mut f = f.unwrap();
    use std::io::Read;
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    hex(&h.finalize())
}

fn hex(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_offline_restart_rename() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    eprintln!("[t] 启动运行时 A…");
    let a = Runtime::start(cfg(da.path(), "集成甲", 45456))
        .await
        .unwrap();
    eprintln!("[t] A 启动完成，启动 B…");
    let b = Runtime::start(cfg(db.path(), "集成乙", 45457))
        .await
        .unwrap();
    eprintln!("[t] B 启动完成，等待互相发现…");
    wait_discovered(&a, &b, 12).await;
    eprintln!("[t] 双向发现完成");

    // 快照在线
    assert!(a
        .peers()
        .iter()
        .any(|p| p.device_id == b.identity.device_id && p.online));
    assert!(b
        .peers()
        .iter()
        .any(|p| p.device_id == a.identity.device_id && p.online));

    // 离线
    b.stop();
    let mut rx = a.events();
    let b_id = b.identity.device_id.clone();
    let off = wait_for(
        &mut rx,
        |e| match e {
            CoreEvent::PeerOffline { device_id, .. } if *device_id == b_id => Some(()),
            _ => None,
        },
        8,
    )
    .await;
    assert!(off.is_some(), "A 未检测到 B 离线");

    // 重启（同数据目录 → 同 device_id，新 instance_id）
    let b2 = Runtime::start(cfg(db.path(), "集成乙", 45457))
        .await
        .unwrap();
    let mut rx2 = a.events();
    let b2_id = b2.identity.device_id.clone();
    let found = wait_for(
        &mut rx2,
        |e| match e {
            CoreEvent::PeerOnline { device_id, .. } if *device_id == b2_id => Some(()),
            _ => None,
        },
        12,
    )
    .await;
    assert!(found.is_some(), "A 未在 B 重启后重新发现");
    assert_eq!(
        b.identity.device_id, b2.identity.device_id,
        "同一数据目录必须保持 device_id"
    );

    // 改名
    b2.rename_device("集成乙-新名").unwrap();
    let mut rx3 = a.events();
    let changed = wait_for(
        &mut rx3,
        |e| match e {
            CoreEvent::PeerNameChanged { new_name, .. } if new_name == "集成乙-新名" => {
                Some(())
            }
            _ => None,
        },
        12,
    )
    .await;
    assert!(changed.is_some(), "A 未看到改名事件");

    a.stop();
    b2.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn text_roundtrip_and_records_encrypted() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    mark("[t2] A 启动");
    let a = Runtime::start(cfg(da.path(), "甲", 45458)).await.unwrap();
    mark("[t2] A 就绪，B 启动");
    let b = Runtime::start(cfg(db.path(), "乙", 45459)).await.unwrap();
    mark("[t2] B 就绪，等待发现");
    wait_discovered(&a, &b, 12).await;
    mark("[t2] 发现完成，connect_to");
    a.connect_to(&b.identity.device_id).await.unwrap();
    mark("[t2] 连接完成，互信");
    trust_both(&a, &b);
    mark("[t2] 互信完成，发送文字");
    // 先订阅再发送：回环传输可能在订阅前就完成，事件不补发
    let mut rx_b2 = b.events();
    let msg_id = a
        .send_text(&b.identity.device_id, "你好，月笺！测试消息")
        .await
        .unwrap();
    let recv = wait_for(
        &mut rx_b2,
        |e| match e {
            CoreEvent::MessageReceived {
                text, message_id, ..
            } if message_id == &msg_id => Some(text.clone()),
            _ => None,
        },
        10,
    )
    .await;
    assert_eq!(recv.as_deref(), Some("你好，月笺！测试消息"));

    // B 的记录已落库且无明文
    let convs = b.conversations().unwrap();
    assert!(convs.iter().any(|c| c.device_id == a.identity.device_id
        && c.messages.iter().any(|m| m.text.contains("你好，月笺"))));
    let raw = std::fs::read(db.path().join("records.db")).unwrap();
    let text = String::from_utf8_lossy(&raw);
    assert!(!text.contains("你好，月笺"), "数据库中不应有明文消息");

    a.stop();
    b.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_transfer_integrity_and_folder() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = Runtime::start(cfg(da.path(), "甲", 45460)).await.unwrap();
    let b = Runtime::start(cfg(db.path(), "乙", 45461)).await.unwrap();
    wait_discovered(&a, &b, 12).await;
    a.connect_to(&b.identity.device_id).await.unwrap();
    trust_both(&a, &b);

    // 准备：一个 5 MiB 随机文件 + 一个文件夹（嵌套子目录）
    let src = da.path().join("大文件.bin");
    let data = rand_bytes(5 * 1024 * 1024);
    std::fs::write(&src, &data).unwrap();
    let folder = da.path().join("照片集");
    std::fs::create_dir_all(folder.join("子目录")).unwrap();
    let f1 = folder.join("图一.jpg");
    let f2 = folder.join("子目录").join("说明 文档.txt");
    std::fs::write(&f1, rand_bytes(300 * 1024)).unwrap();
    let folder_text = "文件夹内文本内容，验证相对路径保留";
    std::fs::write(&f2, folder_text.as_bytes()).unwrap();

    let dest = db.path().join("接收");
    // 先订阅再发送：FileOffer 在本机回环上可能极快到达，broadcast 不补发旧事件。
    let mut rx = b.events();
    let ids = a
        .send_paths(&b.identity.device_id, vec![src.clone(), folder.clone()])
        .await
        .unwrap();
    assert_eq!(ids.len(), 3, "应产生 3 个传输（1 文件 + 2 文件夹内文件）");

    // B 逐个接受
    let mut offered: Vec<String> = Vec::new();
    for _ in 0..3 {
        let id = wait_for(
            &mut rx,
            |e| match e {
                CoreEvent::TransferUpdate(t) if t.state == TransferState::Offered => {
                    Some(t.transfer_id.clone())
                }
                _ => None,
            },
            15,
        )
        .await
        .expect("未收到文件提议");
        if !offered.contains(&id) {
            offered.push(id);
        }
    }
    for id in &offered {
        b.accept_transfer(id, &dest).await.unwrap();
    }

    // 等待全部完成
    let mut rx2 = b.events();
    let mut done = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    while done < 3 && tokio::time::Instant::now() < deadline {
        if let Ok(Ok(CoreEvent::TransferUpdate(t))) =
            tokio::time::timeout(Duration::from_secs(1), rx2.recv()).await
        {
            if offered.contains(&t.transfer_id) && t.state == TransferState::Done {
                done += 1;
            }
        }
    }
    assert_eq!(done, 3, "应有 3 个传输完成");

    // 完整性：逐文件比对 SHA-256
    let got_big = dest.join("大文件.bin");
    eprintln!(
        "[t3] src_exists={} got_exists={} src_len={:?} got_len={:?}",
        src.exists(),
        got_big.exists(),
        std::fs::metadata(&src).map(|m| m.len()),
        std::fs::metadata(&got_big).map(|m| m.len())
    );
    if let Ok(rd) = std::fs::read_dir(&src.parent().unwrap()) {
        for e in rd.flatten() {
            eprintln!("[t3] src 目录内容: {:?}", e.file_name());
        }
    }
    assert!(got_big.exists());
    assert_eq!(sha256_file(&src), sha256_file(&got_big));
    let got_f1 = dest.join("图一.jpg");
    let got_f2 = dest.join("子目录").join("说明 文档.txt");
    assert_eq!(sha256_file(&f1), sha256_file(&got_f1));
    assert_eq!(
        std::fs::read(&got_f2).unwrap(),
        "文件夹内文本内容，验证相对路径保留".as_bytes()
    );

    a.stop();
    b.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn untrusted_sender_file_auto_rejected() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = Runtime::start(cfg(da.path(), "甲", 45462)).await.unwrap();
    let b = Runtime::start(cfg(db.path(), "乙", 45463)).await.unwrap();
    wait_discovered(&a, &b, 12).await;
    a.connect_to(&b.identity.device_id).await.unwrap();
    // 不信任对方

    let src = da.path().join("秘密.bin");
    std::fs::write(&src, rand_bytes(1024)).unwrap();
    // 未信任：发送端直接报错
    let err = a
        .send_paths(&b.identity.device_id, vec![src])
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("未信任"),
        "未信任应拒绝发送: {}",
        err
    );

    a.stop();
    b.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_transfer() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = Runtime::start(cfg(da.path(), "甲", 45464)).await.unwrap();
    let b = Runtime::start(cfg(db.path(), "乙", 45465)).await.unwrap();
    wait_discovered(&a, &b, 12).await;
    a.connect_to(&b.identity.device_id).await.unwrap();
    trust_both(&a, &b);

    let src = da.path().join("取消测试.bin");
    std::fs::write(&src, rand_bytes(64 * 1024 * 1024)).unwrap();
    let dest = db.path().join("接收");
    let ids = a
        .send_paths(&b.identity.device_id, vec![src.clone()])
        .await
        .unwrap();
    let tid = ids[0].clone();

    // B 接受并等待开始传输
    let mut rx = b.events();
    let _ = wait_for(
        &mut rx,
        |e| match e {
            CoreEvent::TransferUpdate(t)
                if t.transfer_id == tid && t.state == TransferState::Offered =>
            {
                Some(())
            }
            _ => None,
        },
        15,
    )
    .await
    .expect("未收到提议");
    b.accept_transfer(&tid, &dest).await.unwrap();
    let _ = wait_for(
        &mut rx,
        |e| match e {
            CoreEvent::TransferUpdate(t) if t.transfer_id == tid && t.transferred > 1024 * 1024 => {
                Some(())
            }
            _ => None,
        },
        60,
    )
    .await
    .expect("传输未开始");

    // A 取消
    a.cancel_transfer(&tid).await.unwrap();
    let canceled = wait_for(
        &mut rx,
        |e| match e {
            CoreEvent::TransferUpdate(t)
                if t.transfer_id == tid && t.state == TransferState::Canceled =>
            {
                Some(())
            }
            _ => None,
        },
        20,
    )
    .await;
    assert!(canceled.is_some(), "B 未收到取消");
    // 无最终文件、无 .part 残留
    assert!(!dest.join("取消测试.bin").exists());
    let leftovers: Vec<_> = std::fs::read_dir(&dest)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".lunote-part"))
        .collect();
    assert!(leftovers.is_empty(), "取消后不应残留临时文件");

    a.stop();
    b.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_after_disconnect() {
    init_tracing();
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = Runtime::start(cfg(da.path(), "甲", 45466)).await.unwrap();
    let b = Runtime::start(cfg(db.path(), "乙", 45467)).await.unwrap();
    wait_discovered(&a, &b, 12).await;
    a.connect_to(&b.identity.device_id).await.unwrap();
    trust_both(&a, &b);

    let src = da.path().join("续传大文件.bin");
    let data = rand_bytes(64 * 1024 * 1024);
    std::fs::write(&src, &data).unwrap();
    let dest = db.path().join("接收");
    let ids = a
        .send_paths(&b.identity.device_id, vec![src.clone()])
        .await
        .unwrap();
    let tid = ids[0].clone();

    let mut rx = b.events();
    let _ = wait_for(
        &mut rx,
        |e| match e {
            CoreEvent::TransferUpdate(t)
                if t.transfer_id == tid && t.state == TransferState::Offered =>
            {
                Some(())
            }
            _ => None,
        },
        15,
    )
    .await
    .expect("未收到提议");
    b.accept_transfer(&tid, &dest).await.unwrap();
    // 等收到 > 8 MiB 再断线
    let _ = wait_for(
        &mut rx,
        |e| match e {
            CoreEvent::TransferUpdate(t)
                if t.transfer_id == tid && t.transferred >= 8 * 1024 * 1024 =>
            {
                Some(())
            }
            _ => None,
        },
        120,
    )
    .await
    .expect("传输未达到 8MiB");

    // 发送端断线
    a.stop();
    let failed = wait_for(
        &mut rx,
        |e| match e {
            CoreEvent::TransferUpdate(t)
                if t.transfer_id == tid && t.state == TransferState::Failed =>
            {
                Some(t.transferred)
            }
            _ => None,
        },
        15,
    )
    .await;
    assert!(failed.is_some(), "B 未标记失败（可续传）");
    let partial_size = failed.unwrap();
    assert!(partial_size > 0, "断线前应有已收数据");

    // 发送端重启（同一数据目录 → 同一 device_id）
    let a2 = Runtime::start(cfg(da.path(), "甲", 45466)).await.unwrap();
    wait_discovered(&a2, &b, 12).await;
    a2.connect_to(&b.identity.device_id).await.unwrap();

    // 重发同一文件 → B 应带 offset 续传
    let ids2 = a2
        .send_paths(&b.identity.device_id, vec![src.clone()])
        .await
        .unwrap();
    let tid2 = ids2[0].clone();
    let mut rx2 = b.events();
    let _ = wait_for(
        &mut rx2,
        |e| match e {
            CoreEvent::TransferUpdate(t)
                if t.transfer_id == tid2 && t.state == TransferState::Offered =>
            {
                Some(())
            }
            _ => None,
        },
        15,
    )
    .await
    .expect("第二次提议未到达");
    b.accept_transfer(&tid2, &dest).await.unwrap();

    // 发送端应显示 resume_offset > 0
    let mut rx3 = a2.events();
    let resumed = wait_for(
        &mut rx3,
        |e| match e {
            CoreEvent::TransferUpdate(t)
                if t.transfer_id == tid2
                    && t.direction == Direction::Outgoing
                    && t.state == TransferState::InProgress
                    && t.resume_offset >= 8 * 1024 * 1024 =>
            {
                Some(t.resume_offset)
            }
            _ => None,
        },
        60,
    )
    .await;
    assert!(resumed.is_some(), "未观察到续传（resume_offset 过小或无）");

    // 等待完成并校验
    let done = wait_for(
        &mut rx2,
        |e| match e {
            CoreEvent::TransferUpdate(t)
                if t.transfer_id == tid2 && t.state == TransferState::Done =>
            {
                Some(())
            }
            _ => None,
        },
        120,
    )
    .await;
    assert!(done.is_some(), "续传未完成");
    let got = dest.join("续传大文件.bin");
    assert!(got.exists(), "接收文件不存在");
    assert_eq!(sha256_file(&src), sha256_file(&got), "续传后文件哈希不一致");
    // 无 .part 残留
    let leftovers: Vec<_> = std::fs::read_dir(&dest)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".lunote-part"))
        .collect();
    assert!(leftovers.is_empty(), "完成后不应残留临时文件");

    a2.stop();
    b.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn export_import_across_instances() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let dc = tempfile::tempdir().unwrap();
    let a = Runtime::start(cfg(da.path(), "甲", 45468)).await.unwrap();
    let b = Runtime::start(cfg(db.path(), "乙", 45469)).await.unwrap();
    wait_discovered(&a, &b, 12).await;
    a.connect_to(&b.identity.device_id).await.unwrap();
    trust_both(&a, &b);
    // 先订阅再发送：回环传输可能在订阅前就完成，事件不补发
    let mut rx = b.events();
    a.send_text(&b.identity.device_id, "跨实例导出测试")
        .await
        .unwrap();
    let _ = wait_for(
        &mut rx,
        |e| match e {
            CoreEvent::MessageReceived { text, .. } if text == "跨实例导出测试" => Some(()),
            _ => None,
        },
        10,
    )
    .await
    .expect("消息未到达");

    // A 导出（A 有 outgoing 记录）
    let out = da.path().join("导出.lunote");
    let report = a.export_records("测试密码123", &out).unwrap();
    assert_eq!(report.messages, 1);

    // C（全新实例）导入
    let c = Runtime::start(cfg(dc.path(), "丙", 45470)).await.unwrap();
    let rep = c.import_records("测试密码123", &out).unwrap();
    assert_eq!(rep.imported_messages, 1);
    let convs = c.conversations().unwrap();
    assert!(convs
        .iter()
        .any(|c| c.messages.iter().any(|m| m.text.contains("跨实例导出测试"))));
    // 再导入一次 → 全去重
    let rep2 = c.import_records("测试密码123", &out).unwrap();
    assert_eq!(rep2.imported_messages, 0);
    assert_eq!(rep2.skipped_messages, 1);
    // 错误密码
    assert!(c.import_records("错误密码", &out).is_err());

    a.stop();
    b.stop();
    c.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_and_resume_transfer() {
    init_tracing();
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = Runtime::start(cfg(da.path(), "暂停甲", 45471))
        .await
        .unwrap();
    let b = Runtime::start(cfg(db.path(), "暂停乙", 45472))
        .await
        .unwrap();
    wait_discovered(&a, &b, 12).await;
    a.connect_to(&b.identity.device_id).await.unwrap();
    trust_both(&a, &b);
    let src = da.path().join("pause.bin");
    std::fs::write(&src, rand_bytes(32 * 1024 * 1024)).unwrap();
    let dest = db.path().join("接收");
    let tid = a
        .send_paths(&b.identity.device_id, vec![src])
        .await
        .unwrap()[0]
        .clone();
    let mut rx = b.events();
    wait_for(
        &mut rx,
        |e| match e {
            CoreEvent::TransferUpdate(t)
                if t.transfer_id == tid && t.state == TransferState::Offered =>
            {
                Some(())
            }
            _ => None,
        },
        15,
    )
    .await
    .expect("未收到暂停测试提议");
    b.accept_transfer(&tid, &dest).await.unwrap();
    let mut arx = a.events();
    wait_for(
        &mut arx,
        |e| match e {
            CoreEvent::TransferUpdate(t)
                if t.transfer_id == tid && t.state == TransferState::InProgress =>
            {
                Some(())
            }
            _ => None,
        },
        15,
    )
    .await
    .expect("暂停测试未开始");
    a.pause_transfer(&tid).await.unwrap();
    wait_for(
        &mut arx,
        |e| match e {
            CoreEvent::TransferUpdate(t)
                if t.transfer_id == tid && t.state == TransferState::Paused =>
            {
                Some(())
            }
            _ => None,
        },
        10,
    )
    .await
    .expect("未收到暂停状态");
    a.resume_transfer(&tid).await.unwrap();
    wait_for(
        &mut rx,
        |e| match e {
            CoreEvent::TransferUpdate(t)
                if t.transfer_id == tid && t.state == TransferState::Done =>
            {
                Some(())
            }
            _ => None,
        },
        90,
    )
    .await
    .expect("继续后传输未完成");
    assert_eq!(
        std::fs::read(dest.join("pause.bin")).unwrap().len(),
        32 * 1024 * 1024
    );
    a.stop();
    b.stop();
}
