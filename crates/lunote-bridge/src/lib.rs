//! lunote-bridge：Flutter UI ↔ lunote-core 的 C ABI 桥。
//!
//! 设计：核心完全独立于 UI。桥只做三件事：
//! - 创建/销毁运行时实例；
//! - 执行 JSON 命令（与 CLI 控制文件使用同一套命令语言）；
//! - 轮询事件（JSON 数组，UI 端定时拉取）。
//!
//! 线程模型：每个实例持有自己的 tokio 运行时；`lunote_call` 为阻塞调用
//! （UI 端在专用 isolate 中调用，不阻塞渲染线程）。

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::{LazyLock, Mutex};

use anyhow::{anyhow, Context, Result};
use lunote_core::{Runtime, RuntimeConfig};

struct Instance {
    runtime: Runtime,
    rt: tokio::runtime::Runtime,
    events_rx: mpsc::Receiver<String>,
    data_dir: PathBuf,
}

static NEXT_ID: AtomicI64 = AtomicI64::new(1);
static INSTANCES: LazyLock<Mutex<HashMap<i64, Instance>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ---------- 生命周期 ----------

/// 创建核心实例；config_json: { "data_dir", "name", "discovery_port", "tcp_port" }
/// 返回句柄（<0 表示失败，-1 错误见 stderr）
#[no_mangle]
pub extern "C" fn lunote_create(config_json: *const c_char) -> i64 {
    let Some(cfg_str) = cstr(config_json) else {
        return -1;
    };
    let (events_tx, events_rx) = mpsc::channel::<String>();
    let result = (|| -> Result<i64> {
        let v: serde_json::Value =
            serde_json::from_str(&cfg_str).context("config_json 解析失败")?;
        let data_dir = v.get("data_dir").and_then(|x| x.as_str()).unwrap_or("data");
        let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("我的设备");
        let discovery_port = v
            .get("discovery_port")
            .and_then(|x| x.as_u64())
            .unwrap_or(45454) as u16;
        let tcp_port = v.get("tcp_port").and_then(|x| x.as_u64()).unwrap_or(45455) as u16;
        let cfg = RuntimeConfig {
            data_dir: PathBuf::from(data_dir),
            name: name.to_string(),
            discovery_port,
            tcp_port,
            ..Default::default()
        };
        // Android 退出 Activity 时进程可能继续存活，而旧 Dart isolate 已丢失句柄。
        // 新 Flutter 引擎重进前先回收同一数据目录的遗留实例，释放 TCP/UDP 端口。
        let stale_instances = {
            let mut instances = INSTANCES.lock().unwrap();
            let stale_ids: Vec<i64> = instances
                .iter()
                .filter_map(|(id, inst)| (inst.data_dir == cfg.data_dir).then_some(*id))
                .collect();
            stale_ids
                .into_iter()
                .filter_map(|id| instances.remove(&id))
                .collect::<Vec<_>>()
        };
        for stale in stale_instances {
            stale.runtime.stop();
            drop(stale);
        }
        let instance_data_dir = cfg.data_dir.clone();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("创建 tokio 运行时失败")?;
        let runtime = rt.block_on(Runtime::start(cfg)).context("核心启动失败")?;
        // 事件转发
        let mut rx = runtime.events();
        std::thread::spawn(move || {
            while let Ok(event) = rx.blocking_recv() {
                if let Ok(line) = serde_json::to_string(&event) {
                    if events_tx.send(line).is_err() {
                        break;
                    }
                }
            }
        });
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        INSTANCES.lock().unwrap().insert(
            id,
            Instance {
                runtime,
                rt,
                events_rx,
                data_dir: instance_data_dir,
            },
        );
        Ok(id)
    })();
    match result {
        Ok(id) => id,
        Err(e) => {
            // 写入核心日志（core.log）便于“一键导出诊断日志”定位；
            // eprintln 只在 logcat/stderr 可见。
            tracing::error!("lunote_create 核心启动失败: {:#}", e);
            eprintln!("lunote_create 失败: {:?}", e);
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn lunote_destroy(handle: i64) {
    if let Some(inst) = INSTANCES.lock().unwrap().remove(&handle) {
        inst.runtime.stop();
    }
}

/// 执行命令（阻塞）。返回 JSON 字符串（调用方用 lunote_free_string 释放）。
#[no_mangle]
pub extern "C" fn lunote_call(handle: i64, cmd_json: *const c_char) -> *mut c_char {
    let Some(cmd) = cstr(cmd_json) else {
        return to_cstr(r#"{"ok":false,"error":"参数为空"}"#);
    };
    let result = (|| -> Result<String> {
        let instances = INSTANCES.lock().unwrap();
        let inst = instances
            .get(&handle)
            .ok_or_else(|| anyhow!("实例不存在: {}", handle))?;
        dispatch(&inst.runtime, &inst.rt, &cmd)
    })();
    match result {
        Ok(s) => to_cstr(&s),
        Err(e) => to_cstr(&format!(
            "{{\"ok\":false,\"error\":{}}}",
            serde_json::to_string(&e.to_string()).unwrap_or_else(|_| "\"未知错误\"".into())
        )),
    }
}

/// 拉取待处理事件；返回 JSON 数组字符串（无事件时返回 "[]"）。
#[no_mangle]
pub extern "C" fn lunote_poll(handle: i64) -> *mut c_char {
    let mut out = Vec::new();
    if let Some(inst) = INSTANCES.lock().unwrap().get(&handle) {
        while let Ok(line) = inst.events_rx.try_recv() {
            out.push(line);
        }
    }
    to_cstr(&format!("[{}]", out.join(",")))
}

#[no_mangle]
pub extern "C" fn lunote_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            drop(CString::from_raw(ptr));
        }
    }
}

// ---------- 命令分发 ----------

fn dispatch(runtime: &Runtime, rt: &tokio::runtime::Runtime, cmd_json: &str) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(cmd_json).context("命令 JSON 解析失败")?;
    let cmd = v.get("cmd").and_then(|x| x.as_str()).unwrap_or("");
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let ok = |extra: &str| Ok::<String, anyhow::Error>(format!("{{\"ok\":true{}}}", extra));
    match cmd {
        "send_text" => {
            let id = rt.block_on(runtime.send_text(&s("device_id"), &s("text")))?;
            ok(&format!(",\"message_id\":\"{}\"", id))
        }
        "send_link" => {
            let id = rt.block_on(runtime.send_link(&s("device_id"), &s("url"), None))?;
            ok(&format!(",\"message_id\":\"{}\"", id))
        }
        "send_file" => {
            let ids =
                rt.block_on(runtime.send_paths(&s("device_id"), vec![PathBuf::from(s("path"))]))?;
            ok(&format!(
                ",\"transfer_ids\":{}",
                serde_json::to_string(&ids)?
            ))
        }
        "trust" => {
            let trusted = v.get("trusted").and_then(|x| x.as_bool()).unwrap_or(true);
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
            runtime.trust_device(&s("device_id"), name, trusted)?;
            ok("")
        }
        "remove_device" => {
            runtime.remove_device(&s("device_id"))?;
            ok("")
        }
        "rename" => {
            runtime.rename_device(&s("name"))?;
            ok("")
        }
        "connect" => {
            rt.block_on(runtime.sessions.connect_to(&s("device_id")))?;
            ok("")
        }
        "connect_address" => {
            let port = v.get("port").and_then(|x| x.as_u64()).unwrap_or(45455);
            if port == 0 || port > u16::MAX as u64 {
                return Err(anyhow!("端口必须在 1 到 65535 之间"));
            }
            let (device_id, name) =
                rt.block_on(runtime.sessions.connect_address(&s("host"), port as u16))?;
            ok(&format!(
                ",\"device_id\":{},\"name\":{}",
                serde_json::to_string(&device_id)?,
                serde_json::to_string(&name)?
            ))
        }
        "accept" => {
            rt.block_on(runtime.accept_transfer(&s("transfer_id"), &PathBuf::from(s("dest"))))?;
            ok("")
        }
        "reject" => {
            rt.block_on(runtime.reject_transfer(&s("transfer_id"), &s("reason")))?;
            ok("")
        }
        "cancel" => {
            rt.block_on(runtime.cancel_transfer(&s("transfer_id")))?;
            ok("")
        }
        "pause" => {
            rt.block_on(runtime.pause_transfer(&s("transfer_id")))?;
            ok("")
        }
        "resume" => {
            rt.block_on(runtime.resume_transfer(&s("transfer_id")))?;
            ok("")
        }
        "export" => {
            let report = runtime.export_records(&s("password"), &PathBuf::from(s("out")))?;
            ok(&format!(
                ",\"messages\":{},\"transfers\":{}",
                report.messages, report.transfers
            ))
        }
        "import" => {
            let report = runtime.import_records(&s("password"), &PathBuf::from(s("input")))?;
            ok(&format!(
                ",\"imported_messages\":{},\"skipped_messages\":{}",
                report.imported_messages, report.skipped_messages
            ))
        }
        "peers" => ok(&format!(
            ",\"peers\":{}",
            serde_json::to_string(&runtime.peers())?
        )),
        "trust_list" => ok(&format!(
            ",\"trusted\":{}",
            serde_json::to_string(&runtime.trust_list())?
        )),
        "transfers" => ok(&format!(
            ",\"transfers\":{}",
            serde_json::to_string(&runtime.transfers())?
        )),
        "conversations" => ok(&format!(
            ",\"conversations\":{}",
            serde_json::to_string(&runtime.conversations()?)?
        )),
        "fingerprint" => match runtime.fingerprint_of(&s("device_id")) {
            Some(fp) => ok(&format!(",\"fingerprint\":\"{}\"", fp)),
            None => Err(anyhow!("设备无记录")),
        },
        "is_trusted" => ok(&format!(
            ",\"trusted\":{}",
            runtime.is_trusted(&s("device_id"))
        )),
        "auto_trust" => ok(&format!(",\"auto_trust\":{}", runtime.auto_trust_enabled())),
        "set_auto_trust" => {
            let enabled = v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);
            runtime.set_auto_trust(enabled)?;
            ok("")
        }
        "settings" => ok(&format!(
            ",\"settings\":{}",
            serde_json::to_string(&runtime.settings())?
        )),
        "set_downloads_dir" => {
            let dir = v.get("dir").and_then(|x| x.as_str());
            runtime.set_downloads_dir(dir)?;
            ok("")
        }
        "set_theme" => {
            let theme = v.get("theme").and_then(|x| x.as_str()).unwrap_or("dark");
            runtime.set_theme(theme)?;
            ok("")
        }
        "set_conflict_policy" => {
            let policy = v.get("policy").and_then(|x| x.as_str()).unwrap_or("rename");
            runtime.set_conflict_policy(policy)?;
            ok("")
        }
        "set_receive_tree_uri" => {
            let uri = v.get("uri").and_then(|x| x.as_str());
            runtime.set_receive_tree_uri(uri)?;
            ok("")
        }
        "set_pin" => {
            let pin = v.get("pin").and_then(|x| x.as_str());
            runtime.set_pin(pin)?;
            ok("")
        }
        "verify_pin" => {
            let pin = v.get("pin").and_then(|x| x.as_str()).unwrap_or("");
            ok(&format!(",\"valid\":{}", runtime.verify_pin(pin)))
        }
        "set_background" => {
            let bg = v
                .get("background")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            runtime.set_background(bg);
            ok("")
        }
        "wipe_records" => {
            runtime.wipe_records()?;
            ok("")
        }
        "delete_conversation" => {
            runtime.delete_conversation(&s("device_id"))?;
            ok("")
        }
        "delete_conversations" => {
            let ids = v
                .get("device_ids")
                .and_then(|x| x.as_array())
                .ok_or_else(|| anyhow!("device_ids 必须是数组"))?
                .iter()
                .map(|x| {
                    x.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| anyhow!("device_ids 只能包含字符串"))
                })
                .collect::<Result<Vec<_>>>()?;
            runtime.delete_conversations(&ids)?;
            ok("")
        }
        "identity" => ok(&format!(
            ",\"device_id\":\"{}\",\"name\":\"{}\",\"instance_id\":\"{}\"",
            runtime.identity.device_id,
            runtime.identity.name(),
            runtime.identity.instance_id
        )),
        "data_dir" => ok(&format!(
            ",\"data_dir\":\"{}\"",
            runtime
                .identity
                .data_dir()
                .to_string_lossy()
                .replace('\\', "/")
        )),
        "diagnostics" => ok(&format!(
            ",\"diagnostics\":{}",
            serde_json::to_string(&runtime.diagnostics())?
        )),
        _ => Err(anyhow!("未知命令: {}", cmd)),
    }
}

// ---------- 工具 ----------

fn cstr(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .ok()
        .map(|s| s.to_string())
}

fn to_cstr(s: &str) -> *mut c_char {
    CString::new(s)
        .map(|c| c.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// 测试入口（cargo test 直接调用命令层）
pub fn call_command(config: &str, commands: &[&str]) -> Vec<String> {
    let config = CString::new(config).unwrap();
    let handle = lunote_create(config.as_ptr());
    assert!(handle > 0, "核心启动失败");
    let mut out = Vec::new();
    for cmd in commands {
        let cmd = CString::new(*cmd).unwrap();
        let ptr = lunote_call(handle, cmd.as_ptr());
        let result = if ptr.is_null() {
            "null".to_string()
        } else {
            unsafe { CStr::from_ptr(ptr).to_string_lossy().to_string() }
        };
        if !ptr.is_null() {
            lunote_free_string(ptr);
        }
        out.push(result);
    }
    lunote_destroy(handle);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free_tcp_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn create(config: &str) -> i64 {
        let config = CString::new(config).unwrap();
        lunote_create(config.as_ptr())
    }

    fn call(handle: i64, command: &str) -> serde_json::Value {
        let command = CString::new(command).unwrap();
        let ptr = lunote_call(handle, command.as_ptr());
        assert!(!ptr.is_null());
        let raw = unsafe { CStr::from_ptr(ptr).to_string_lossy().to_string() };
        lunote_free_string(ptr);
        serde_json::from_str(&raw).unwrap()
    }

    #[test]
    fn bridge_identity_and_commands() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = format!(
            "{{\"data_dir\":\"{}\",\"name\":\"桥测试\",\"tcp_port\":{}}}",
            dir.path().display().to_string().replace('\\', "\\\\"),
            free_tcp_port()
        );
        let results = call_command(
            &cfg,
            &[
                r#"{"cmd":"identity"}"#,
                r#"{"cmd":"peers"}"#,
                r#"{"cmd":"trust_list"}"#,
                r#"{"cmd":"send_text","device_id":"x","text":"y"}"#,
            ],
        );
        assert!(results[0].contains("\"ok\":true"), "{}", results[0]);
        assert!(
            results[3].contains("\"ok\":false"),
            "无设备应失败: {}",
            results[3]
        );
    }

    #[test]
    fn settings_and_name_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = format!(
            "{{\"data_dir\":\"{}\",\"name\":\"默认名\",\"tcp_port\":{}}}",
            dir.path().display().to_string().replace('\\', "\\\\"),
            free_tcp_port()
        );
        let first = create(&cfg);
        assert!(first > 0);
        assert_eq!(
            call(first, r#"{"cmd":"set_theme","theme":"light"}"#)["ok"],
            true
        );
        assert_eq!(
            call(first, r#"{"cmd":"set_auto_trust","enabled":false}"#)["ok"],
            true
        );
        assert_eq!(
            call(first, r#"{"cmd":"rename","name":"持久名称"}"#)["ok"],
            true
        );
        lunote_destroy(first);

        let second = create(&cfg);
        assert!(second > 0);
        let settings = call(second, r#"{"cmd":"settings"}"#);
        assert_eq!(settings["settings"]["theme"], "light");
        assert_eq!(settings["settings"]["auto_trust"], false);
        assert_eq!(call(second, r#"{"cmd":"identity"}"#)["name"], "持久名称");
        lunote_destroy(second);
    }

    #[test]
    fn recreate_same_data_dir_reclaims_stale_handle() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = format!(
            "{{\"data_dir\":\"{}\",\"name\":\"重进测试\",\"tcp_port\":{}}}",
            dir.path().display().to_string().replace('\\', "\\\\"),
            free_tcp_port()
        );
        let stale = create(&cfg);
        assert!(stale > 0);
        let replacement = create(&cfg);
        assert!(replacement > 0, "同进程重进应主动回收遗留实例");
        assert_eq!(call(stale, r#"{"cmd":"identity"}"#)["ok"], false);
        assert_eq!(call(replacement, r#"{"cmd":"identity"}"#)["ok"], true);
        lunote_destroy(replacement);
    }

    #[test]
    fn connects_directly_by_address_and_returns_verified_identity() {
        let first_dir = tempfile::tempdir().unwrap();
        let second_dir = tempfile::tempdir().unwrap();
        let first_tcp = free_tcp_port();
        let second_tcp = free_tcp_port();
        let first_discovery = free_tcp_port();
        let second_discovery = free_tcp_port();
        let first_cfg = format!(
            "{{\"data_dir\":\"{}\",\"name\":\"直连甲\",\"tcp_port\":{},\"discovery_port\":{}}}",
            first_dir.path().display().to_string().replace('\\', "\\\\"),
            first_tcp,
            first_discovery
        );
        let second_cfg = format!(
            "{{\"data_dir\":\"{}\",\"name\":\"直连乙\",\"tcp_port\":{},\"discovery_port\":{}}}",
            second_dir
                .path()
                .display()
                .to_string()
                .replace('\\', "\\\\"),
            second_tcp,
            second_discovery
        );
        let first = create(&first_cfg);
        let second = create(&second_cfg);
        assert!(first > 0 && second > 0);
        let expected = call(second, r#"{"cmd":"identity"}"#)["device_id"]
            .as_str()
            .unwrap()
            .to_string();
        let connected = call(
            first,
            &format!(
                r#"{{"cmd":"connect_address","host":"127.0.0.1","port":{}}}"#,
                second_tcp
            ),
        );
        assert_eq!(connected["ok"], true, "{connected}");
        assert_eq!(connected["device_id"], expected);
        assert_eq!(connected["name"], "直连乙");
        lunote_destroy(first);
        lunote_destroy(second);
    }
}
