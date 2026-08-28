//! 月笺 Lunote 无头 CLI：核心自动化测试与回归测试入口。
//!
//! 模式：
//! - `serve`：运行核心，事件以 JSON 行写入 event_file；支持 control_file
//!   （追加 JSON 行命令）驱动，供自动化测试使用；
//! - 一次性命令：`peers` / `trust-list` / `trust` / `send-text` / `send-file` /
//!   `accept` / `reject` / `cancel` / `export-records` / `import-records`。

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use lunote_core::events::{CoreEvent, TransferState};
use lunote_core::{Runtime, RuntimeConfig};

#[derive(Parser)]
#[command(name = "lunote-cli", about = "月笺 Lunote 无头核心（自动化/回归测试）")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 运行核心（前台），事件写 JSON 行；可选控制文件驱动
    Serve {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "data")]
        data_dir: String,
        #[arg(long, default_value_t = 45454)]
        discovery_port: u16,
        #[arg(long, default_value_t = 45455)]
        tcp_port: u16,
        #[arg(long)]
        event_file: Option<String>,
        #[arg(long)]
        control_file: Option<String>,
        #[arg(long)]
        background: bool,
    },
    /// 列出当前发现的设备（运行 4 秒采样）
    Peers {
        #[arg(long, default_value = "data")]
        data_dir: String,
    },
    /// 列出信任设备
    TrustList {
        #[arg(long, default_value = "data")]
        data_dir: String,
    },
    /// 信任 / 取消信任设备
    Trust {
        #[arg(long, default_value = "data")]
        data_dir: String,
        #[arg(long)]
        device_id: String,
        #[arg(long, default_value_t = true)]
        trusted: bool,
    },
    /// 发送文字（等待设备上线后发送）
    SendText {
        #[arg(long, default_value = "data")]
        data_dir: String,
        #[arg(long)]
        device_id: String,
        #[arg(long)]
        text: String,
        #[arg(long, default_value = "8")]
        wait_secs: u64,
    },
    /// 发送文件 / 文件夹
    SendFile {
        #[arg(long, default_value = "data")]
        data_dir: String,
        #[arg(long)]
        device_id: String,
        #[arg(long)]
        path: String,
        #[arg(long, default_value = "8")]
        wait_secs: u64,
    },
    /// 接受收到的文件（dest 为保存目录）
    Accept {
        #[arg(long, default_value = "data")]
        data_dir: String,
        #[arg(long)]
        transfer_id: String,
        #[arg(long)]
        dest: String,
    },
    /// 拒绝收到的文件
    Reject {
        #[arg(long, default_value = "data")]
        data_dir: String,
        #[arg(long)]
        transfer_id: String,
    },
    /// 取消传输
    Cancel {
        #[arg(long, default_value = "data")]
        data_dir: String,
        #[arg(long)]
        transfer_id: String,
    },
    /// 导出加密记录
    ExportRecords {
        #[arg(long, default_value = "data")]
        data_dir: String,
        #[arg(long)]
        password: String,
        #[arg(long)]
        out: String,
    },
    /// 导入加密记录
    ImportRecords {
        #[arg(long, default_value = "data")]
        data_dir: String,
        #[arg(long)]
        password: String,
        #[arg(long)]
        input: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Serve {
            name,
            data_dir,
            discovery_port,
            tcp_port,
            event_file,
            control_file,
            background,
        } => {
            serve(
                name,
                data_dir,
                discovery_port,
                tcp_port,
                event_file,
                control_file,
                background,
            )
            .await
        }
        Cmd::Peers { data_dir } => peers(&data_dir).await,
        Cmd::TrustList { data_dir } => trust_list(&data_dir),
        Cmd::Trust {
            data_dir,
            device_id,
            trusted,
        } => trust(&data_dir, &device_id, trusted),
        Cmd::SendText {
            data_dir,
            device_id,
            text,
            wait_secs,
        } => send_text(&data_dir, &device_id, &text, wait_secs).await,
        Cmd::SendFile {
            data_dir,
            device_id,
            path,
            wait_secs,
        } => send_file(&data_dir, &device_id, &path, wait_secs).await,
        Cmd::Accept {
            data_dir,
            transfer_id,
            dest,
        } => accept(&data_dir, &transfer_id, &dest).await,
        Cmd::Reject {
            data_dir,
            transfer_id,
        } => reject(&data_dir, &transfer_id).await,
        Cmd::Cancel {
            data_dir,
            transfer_id,
        } => cancel(&data_dir, &transfer_id).await,
        Cmd::ExportRecords {
            data_dir,
            password,
            out,
        } => export_records(&data_dir, &password, &out),
        Cmd::ImportRecords {
            data_dir,
            password,
            input,
        } => import_records(&data_dir, &password, &input),
    }
}

fn cfg(data_dir: &str, name: &str, discovery_port: u16, tcp_port: u16) -> RuntimeConfig {
    RuntimeConfig {
        data_dir: PathBuf::from(data_dir),
        name: name.to_string(),
        discovery_port,
        tcp_port,
        discovery_interval: Duration::from_millis(500),
        offline_timeout: Duration::from_secs(2),
        downloads_dir: None,
        background: false,
    }
}

async fn serve(
    name: String,
    data_dir: String,
    discovery_port: u16,
    tcp_port: u16,
    event_file: Option<String>,
    control_file: Option<String>,
    background: bool,
) -> Result<()> {
    let runtime = Runtime::start(cfg(&data_dir, &name, discovery_port, tcp_port)).await?;
    runtime.set_background(background);
    println!(
        "{{ \"event\": \"cli_started\", \"device_id\": \"{}\", \"name\": \"{}\" }}",
        runtime.identity.device_id,
        runtime.identity.name()
    );

    let mut events_out: Option<std::fs::File> = match &event_file {
        Some(path) => {
            let f = std::fs::File::create(path)
                .with_context(|| format!("无法创建事件文件 {}", path))?;
            Some(f)
        }
        None => None,
    };

    // 控制文件驱动（可能稍后创建，轮询等待）
    let control_path = control_file.map(PathBuf::from);
    let mut control_reader: Option<ControlReader> = None;
    let mut last_line: u64 = 0;

    let mut rx = runtime.events();
    loop {
        // 处理控制文件
        if control_path.is_some() {
            if control_reader.is_none() {
                if control_path.as_ref().unwrap().exists() {
                    control_reader = Some(ControlReader::open(control_path.as_ref().unwrap()));
                }
            }
            if let Some(reader) = control_reader.as_mut() {
                for line in reader.read_new(&mut last_line)? {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let v: serde_json::Value = serde_json::from_str(&line)?;
                    if v["quit"].as_bool() == Some(true) {
                        println!("{{ \"event\": \"cli_quit\" }}");
                        runtime.stop();
                        return Ok(());
                    }
                    handle_command(&runtime, &v).await;
                }
            }
        }
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(event)) => {
                let line = serde_json::to_string(&event)?;
                if let Some(f) = events_out.as_mut() {
                    use std::io::Write;
                    writeln!(f, "{}", line)?;
                    f.flush()?;
                }
                println!("{}", line);
            }
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }
    Ok(())
}

struct ControlReader {
    file: std::fs::File,
}

impl ControlReader {
    fn open(path: &PathBuf) -> Self {
        // Windows 下默认 File::open 只共享读，外部进程无法追加写控制文件；
        // 显式声明共享读/写/删，让控制文件真正可用（如 PowerShell Add-Content）。
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_SHARE_READ: u32 = 0x1;
            const FILE_SHARE_WRITE: u32 = 0x2;
            const FILE_SHARE_DELETE: u32 = 0x4;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(path)
                .expect("无法打开控制文件");
            Self { file }
        }
        #[cfg(not(windows))]
        {
            Self {
                file: std::fs::File::open(path).expect("无法打开控制文件"),
            }
        }
    }
    /// 读取新增行（基于文件长度增量）
    fn read_new(&mut self, last: &mut u64) -> Result<Vec<String>> {
        use std::io::{Read, Seek, SeekFrom};
        let len = self.file.metadata()?.len();
        if len <= *last {
            return Ok(Vec::new());
        }
        self.file.seek(SeekFrom::Start(*last))?;
        let mut buf = Vec::new();
        self.file.read_to_end(&mut buf)?;
        *last = len;
        let text = String::from_utf8_lossy(&buf);
        Ok(text.lines().map(|s| s.to_string()).collect())
    }
}

async fn handle_command(runtime: &Runtime, v: &serde_json::Value) {
    let cmd = v["cmd"].as_str().unwrap_or("");
    let result = match cmd {
        "send_text" => {
            let device_id = v["device_id"].as_str().unwrap_or("");
            let text = v["text"].as_str().unwrap_or("");
            runtime.send_text(device_id, text).await.map(|id| {
                format!(
                    "{{\"cmd\":\"send_text\",\"ok\":true,\"message_id\":\"{}\"}}",
                    id
                )
            })
        }
        "send_link" => {
            let device_id = v["device_id"].as_str().unwrap_or("");
            let url = v["url"].as_str().unwrap_or("");
            runtime.send_link(device_id, url, None).await.map(|id| {
                format!(
                    "{{\"cmd\":\"send_link\",\"ok\":true,\"message_id\":\"{}\"}}",
                    id
                )
            })
        }
        "send_file" => {
            let device_id = v["device_id"].as_str().unwrap_or("");
            let path = v["path"].as_str().unwrap_or("");
            match runtime
                .send_paths(device_id, vec![PathBuf::from(path)])
                .await
            {
                Ok(ids) => serde_json::to_string(&ids)
                    .map(|s| {
                        format!(
                            "{{\"cmd\":\"send_file\",\"ok\":true,\"transfer_ids\":{}}}",
                            s
                        )
                    })
                    .map_err(|e| anyhow::anyhow!(e)),
                Err(e) => Err(e),
            }
        }
        "trust" => {
            let device_id = v["device_id"].as_str().unwrap_or("");
            let trusted = v["trusted"].as_bool().unwrap_or(true);
            let name = v["name"].as_str().unwrap_or("");
            runtime
                .trust_device(device_id, name, trusted)
                .map(|_| format!("{{\"cmd\":\"trust\",\"ok\":true}}"))
        }
        "rename" => {
            let name = v["name"].as_str().unwrap_or("");
            runtime
                .rename_device(name)
                .map(|_| format!("{{\"cmd\":\"rename\",\"ok\":true}}"))
        }
        "settings" => Ok(format!(
            "{{\"cmd\":\"settings\",\"ok\":true,\"settings\":{}}}",
            serde_json::to_string(&runtime.settings()).unwrap_or_default()
        )),
        "set_downloads_dir" => {
            let dir = v["dir"].as_str();
            runtime
                .set_downloads_dir(dir)
                .map(|_| format!("{{\"cmd\":\"set_downloads_dir\",\"ok\":true}}"))
        }
        "connect" => {
            let device_id = v["device_id"].as_str().unwrap_or("");
            runtime
                .connect_to(device_id)
                .await
                .map(|_| format!("{{\"cmd\":\"connect\",\"ok\":true}}"))
        }
        "accept" => {
            let transfer_id = v["transfer_id"].as_str().unwrap_or("");
            let dest = v["dest"].as_str().unwrap_or("");
            runtime
                .accept_transfer(transfer_id, &PathBuf::from(dest))
                .await
                .map(|_| format!("{{\"cmd\":\"accept\",\"ok\":true}}"))
        }
        "reject" => {
            let transfer_id = v["transfer_id"].as_str().unwrap_or("");
            let reason = v["reason"].as_str().unwrap_or("用户拒绝");
            runtime
                .reject_transfer(transfer_id, reason)
                .await
                .map(|_| format!("{{\"cmd\":\"reject\",\"ok\":true}}"))
        }
        "cancel" => {
            let transfer_id = v["transfer_id"].as_str().unwrap_or("");
            runtime
                .cancel_transfer(transfer_id)
                .await
                .map(|_| format!("{{\"cmd\":\"cancel\",\"ok\":true}}"))
        }
        "export" => {
            let password = v["password"].as_str().unwrap_or("");
            let out = v["out"].as_str().unwrap_or("");
            runtime
                .export_records(password, &PathBuf::from(out))
                .map(|r| {
                    format!(
                        "{{\"cmd\":\"export\",\"ok\":true,\"messages\":{},\"transfers\":{}}}",
                        r.messages, r.transfers
                    )
                })
        }
        "import" => {
            let password = v["password"].as_str().unwrap_or("");
            let input = v["input"].as_str().unwrap_or("");
            runtime.import_records(password, &PathBuf::from(input)).map(|r| format!("{{\"cmd\":\"import\",\"ok\":true,\"imported_messages\":{},\"skipped_messages\":{}}}", r.imported_messages, r.skipped_messages))
        }
        "list_transfers" => serde_json::to_string(&runtime.transfers())
            .map(|s| {
                format!(
                    "{{\"cmd\":\"list_transfers\",\"ok\":true,\"transfers\":{}}}",
                    s
                )
            })
            .map_err(|e| anyhow::anyhow!(e)),
        "status" => {
            let peers = serde_json::to_string(&runtime.peers());
            let trusted = serde_json::to_string(&runtime.trust_list());
            match (peers, trusted) {
                (Ok(p), Ok(t)) => Ok(format!(
                    "{{\"cmd\":\"status\",\"ok\":true,\"peers\":{},\"trusted\":{}}}",
                    p, t
                )),
                (Err(e), _) | (_, Err(e)) => Err(anyhow::anyhow!(e)),
            }
        }
        _ => Ok(format!(
            "{{\"cmd\":\"{}\",\"ok\":false,\"error\":\"未知命令\"}}",
            cmd
        )),
    };
    match result {
        Ok(line) => println!("{}", line),
        Err(e) => println!(
            "{{\"cmd\":\"{}\",\"ok\":false,\"error\":{}}}",
            cmd,
            serde_json::to_string(&e.to_string()).unwrap_or_default()
        ),
    }
}

async fn peers(data_dir: &str) -> Result<()> {
    let runtime = Runtime::start(cfg(data_dir, "lunote-cli-sampler", 45454, 45459)).await?;
    tokio::time::sleep(Duration::from_secs(4)).await;
    println!("{}", serde_json::to_string_pretty(&runtime.peers())?);
    runtime.stop();
    Ok(())
}

fn trust_list(data_dir: &str) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?
        .block_on(Runtime::start(cfg(data_dir, "x", 45454, 45459)))?;
    println!("{}", serde_json::to_string_pretty(&runtime.trust_list())?);
    runtime.stop();
    Ok(())
}

fn trust(data_dir: &str, device_id: &str, trusted: bool) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?
        .block_on(Runtime::start(cfg(data_dir, "x", 45454, 45459)))?;
    runtime.trust_device(device_id, "", trusted)?;
    println!(
        "{{ \"ok\": true, \"device_id\": \"{}\", \"trusted\": {} }}",
        device_id, trusted
    );
    runtime.stop();
    Ok(())
}

async fn wait_peer(runtime: &Runtime, device_id: &str, secs: u64) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if runtime
            .peers()
            .iter()
            .any(|p| p.device_id == device_id && p.online)
        {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("等待设备上线超时: {}", device_id);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn send_text(data_dir: &str, device_id: &str, text: &str, wait_secs: u64) -> Result<()> {
    let runtime = Runtime::start(cfg(data_dir, "lunote-cli", 45454, 45455)).await?;
    wait_peer(&runtime, device_id, wait_secs).await?;
    let id = runtime.send_text(device_id, text).await?;
    println!("{{ \"ok\": true, \"message_id\": \"{}\" }}", id);
    runtime.stop();
    Ok(())
}

async fn send_file(data_dir: &str, device_id: &str, path: &str, wait_secs: u64) -> Result<()> {
    let runtime = Runtime::start(cfg(data_dir, "lunote-cli", 45454, 45455)).await?;
    wait_peer(&runtime, device_id, wait_secs).await?;
    let ids = runtime
        .send_paths(device_id, vec![PathBuf::from(path)])
        .await?;
    println!(
        "{{ \"ok\": true, \"transfer_ids\": {} }}",
        serde_json::to_string(&ids)?
    );
    // 等待结束
    let mut rx = runtime.events();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    let mut done = 0;
    while done < ids.len() && tokio::time::Instant::now() < deadline {
        if let Ok(event) = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
            if let Ok(CoreEvent::TransferUpdate(t)) = event {
                if ids.contains(&t.transfer_id)
                    && matches!(
                        t.state,
                        TransferState::Done
                            | TransferState::Failed
                            | TransferState::Canceled
                            | TransferState::Rejected
                    )
                {
                    println!("{}", serde_json::to_string(&t)?);
                    done += 1;
                }
            }
        }
    }
    runtime.stop();
    Ok(())
}

async fn accept(data_dir: &str, transfer_id: &str, dest: &str) -> Result<()> {
    let runtime = Runtime::start(cfg(data_dir, "lunote-cli", 45454, 45455)).await?;
    runtime
        .accept_transfer(transfer_id, &PathBuf::from(dest))
        .await?;
    println!("{{ \"ok\": true }}");
    runtime.stop();
    Ok(())
}

async fn reject(data_dir: &str, transfer_id: &str) -> Result<()> {
    let runtime = Runtime::start(cfg(data_dir, "lunote-cli", 45454, 45455)).await?;
    runtime.reject_transfer(transfer_id, "用户拒绝").await?;
    println!("{{ \"ok\": true }}");
    runtime.stop();
    Ok(())
}

async fn cancel(data_dir: &str, transfer_id: &str) -> Result<()> {
    let runtime = Runtime::start(cfg(data_dir, "lunote-cli", 45454, 45455)).await?;
    runtime.cancel_transfer(transfer_id).await?;
    println!("{{ \"ok\": true }}");
    runtime.stop();
    Ok(())
}

fn export_records(data_dir: &str, password: &str, out: &str) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?
        .block_on(Runtime::start(cfg(data_dir, "x", 45454, 45459)))?;
    let report = runtime.export_records(password, &PathBuf::from(out))?;
    println!(
        "{{ \"ok\": true, \"messages\": {}, \"transfers\": {} }}",
        report.messages, report.transfers
    );
    runtime.stop();
    Ok(())
}

fn import_records(data_dir: &str, password: &str, input: &str) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?
        .block_on(Runtime::start(cfg(data_dir, "x", 45454, 45459)))?;
    let report = runtime.import_records(password, &PathBuf::from(input))?;
    println!(
        "{{ \"ok\": true, \"imported_messages\": {}, \"skipped_messages\": {} }}",
        report.imported_messages, report.skipped_messages
    );
    runtime.stop();
    Ok(())
}
