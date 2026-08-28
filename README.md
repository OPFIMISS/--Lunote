# 月笺 Lunote

当前版本：**1.2.0（3）**。保持 applicationId 与签名配置不变，支持覆盖更新 APK。

无云端、无账号、无互联网依赖的局域网通信与文件传输应用。
Android / Windows 10+ / Linux，设备直连（E2E），默认加密，本地加密记录。

文件冲突策略支持自动重命名、覆盖或跳过，并在 PC/Android 双端持久化；设置页提供设备诊断面板，可查看监听端口、在线设备、发现统计和收发目录。

设置页还支持可选应用锁：应用从后台恢复时重新锁定，核心只保存 PIN 的 SHA-256 摘要。

## 项目截图

### Android

![Android 首页](docs/screenshots/android-start.png)

![Android 设置](docs/screenshots/android-settings.png)

![Android 接收目录](docs/screenshots/android-receive-directory.png)

### Windows

![Windows 首页](docs/screenshots/windows-start.png)

## 发布

发布页资产由 tools/publish_github_release.ps1 校验并上传。配置 GitHub CLI 后执行 gh auth login，再运行该脚本。

- 项目根目录：`D:\Lunote 2\moonletter`
- 技术栈：Rust（核心：tokio + rustls 1.3 + Ed25519 + AES-256-GCM）＋ Flutter（UI）
- 详细说明：`docs/`（计划与架构、协议、安全模型、构建说明、开发路径指南、交付报告）

---

## 1. 目录结构与文件职责（维护必读）

```
moonletter/
├── crates/
│   ├── lunote-core/            ★核心功能（改坏 = 所有端全挂，务必小心）
│   │   └── src/
│   │       ├── discovery.rs    设备发现（UDP 组播/广播、离线、重启替换、统计）
│   │       ├── identity.rs     设备身份（Ed25519 密钥 + 证书 + 指纹）
│   │       ├── trust.rs        信任与 TOFU 记录（trust.json、自动信任判定）
│   │       ├── session.rs      TLS 1.3 会话（握手、心跳、读写任务、消息分发）
│   │       ├── transfer.rs     文件/文件夹传输（分块、校验、断点续传）
│   │       ├── store.rs        本地加密记录（SQLite + AES-256-GCM、导出/导入）
│   │       ├── events.rs       事件定义（UI 的数据来源，字段别乱改）
│   │       ├── runtime.rs      Runtime 聚合（启动、命令实现、设置持久化）
│   │       ├── platform.rs     平台工具
│   │       └── messages.rs     消息模型
│   ├── lunote-bridge/           ★FFI 命令层（UI 与核心的唯一入口）
│   │   └── src/lib.rs           命令分发（JSON 命令 → Runtime 方法）
│   └── lunote-cli/              无头核心（serve/peers/trust/send-text/…，测试与调试）
├── app/                        Flutter UI
│   ├── lib/
│   │   ├── main.dart            入口：窗口/托盘/初始化（桌面平台）
│   │   └── src/
│   │       ├── core/            ★FFI 桥与事件流（改坏 = 整个 UI 失灵）
│   │       │   ├── core_bridge.dart   原生库加载与 FFI 绑定
│   │       │   ├── core_client.dart   事件轮询 worker + 命令调用
│   │       │   ├── models.dart        数据模型（事件/命令的 JSON 映射）
│   │       │   └── window_ui.dart     UI 诊断日志（%TEMP%\lunote_ui.log）
│   │       ├── state/
│   │       │   └── app_state.dart     全局状态（设备/信任/消息/传输/设置）
│   │       └── ui/              纯 UI（★可放心大改，只与 AppState/模型交互）
│   │           ├── lunote_theme.dart  主题（颜色/弹簧参数）
│   │           ├── pages/
│   │           │   ├── shell_page.dart    应用壳（宽屏侧栏/窄屏底部导航）
│   │           │   ├── devices_page.dart  设备页（发现列表/信任/刷新）
│   │           │   ├── chat_page.dart     对话页（消息/文件/返回）
│   │           │   ├── transfers_page.dart 传输中心
│   │           │   └── settings_page.dart 设置（改名/导出/自动信任/诊断日志）
│   │           └── widgets/     通用组件（SpringButton、MessageBubble…）
│   ├── android/  windows/  linux/   各平台壳与配置（一般不用动）
│   ├── integration_test/      真实 App 集成测试（改 UI 后必跑）
│   └── plugins/desktop_drop    vendored 补丁插件（勿动）
├── tests/regression_cli.py     跨进程网络回归（真实双 CLI 进程）
├── tools/build_all.ps1         一键构建（Windows + Android → dist/）
├── docs/                       全部设计/协议/安全/构建/踩坑文档
└── dist/                        ★发布产物统一输出目录（见第 6 节）
```

### 文件危险等级

| 等级 | 文件 | 说明 |
| --- | --- | --- |
| 🔴 核心 | `crates/lunote-core/**` | 协议与状态机。改动必须跑 `cargo test`（16 单元 + 7 e2e） |
| 🔴 核心 | `app/lib/src/core/**` | UI 与核心的桥。改动必须跑集成测试（否则 UI 全失灵且无报错） |
| 🟡 边界 | `crates/lunote-bridge`、`app/lib/src/state/app_state.dart` | 改命令/事件/状态时两端字段必须一致（snake_case） |
| 🟢 可大改 | `app/lib/src/ui/**`（pages/widgets/theme） | **UI 重做就改这里**，只要通过 AppState/CoreClient 拿数据、调用方法 |

---

## 2. UI 与核心的通信（大改 UI 前必须懂）

UI **不能**直接碰协议/网络，一切通过两层接口：

```
UI (pages)  →  AppState (状态)  →  CoreClient (worker isolate)  →  lunote_bridge.dll  →  Rust Runtime
事件返回:    ←   notifyListeners  ←   事件流 poll (150ms)      ←   ←
```

- **命令**（UI 调核心）：`core.call('命令名', {参数})`，返回 `{"ok":true,...}` 或 `{"ok":false,"error":"..."}`
  - 常用：`identity` `peers` `trust_list` `trust` `rename` `send_text` `send_link` `send_file`
    `accept` `reject` `cancel` `export` `import` `wipe_records` `data_dir`
    `auto_trust` `set_auto_trust` `set_background` `conversations` `transfers`
  - 字段一律 snake_case（`device_id`、`transfer_id`、`ts_ms`）
- **事件**（核心推 UI）：`core.events` 流，`e['event']` 区分：
  - `peer_online` / `peer_offline` / `peer_name_changed` / `peer_connected`（含 `trusted`/`is_new_device`/`auto_trusted`）
  - `identity_changed` / `trust_changed` / `message_received` / `message_sent`
  - `transfer_update` / `records_changed`
- **铁律**：
  1. 事件可能丢（启动初期）→ 启动后主动拉快照 + 每 3 秒轮询（AppState 已实现，勿删）
  2. worker isolate 是独立静态区 → 改动 CoreClient 时必须在 worker 里重新初始化它用的单例
  3. 新增事件/命令字段时，Rust 与 Dart 模型必须同步

---

## 3. 主要功能区

| 页面 | 文件 | 职责 | 数据来源 |
| --- | --- | --- | --- |
| 设备 | `devices_page.dart` | 发现列表（在线/离线/信任/指纹警告）、信任操作（确认生效+反馈）、刷新 | `state.peers` + `state.trusted` |
| 对话 | `chat_page.dart` | 消息/链接/文件/文件夹、拖放发送、传输卡片操作、返回（内嵌 onBack / push） | `state.messagesOf/transfersOf` |
| 传输 | `transfers_page.dart` | 全部传输状态中心 | `state.allTransfers` |
| 设置 | `settings_page.dart` | 设备名（读回核对）、记录导出/导入/删除、**接收文件保存位置**、**主题外观（深/浅/跟随系统）**、自动信任开关、诊断日志导出 | `state.*` + `core.call` |
| 壳 | `shell_page.dart` | 宽屏侧栏 / 窄屏底部导航、对话进入、信任弹窗、**对话长按/右键删除** | — |

## 4. 数据与日志位置

| 项目 | 位置 |
| --- | --- |
| PC 数据目录 | `%APPDATA%\com.lunote\lunote_app\data\`（identity/密钥/trust.json/records.db/core.log/settings.json） |
| Android 数据目录 | App 私有目录（同结构） |
| 核心日志 | `data/core.log`（UTC 时间戳！本地 = +8；超过 2MB 滚动 core.1.log） |
| UI 日志 | `%TEMP%\lunote_ui.log`（窗口操作/导出埋点） |
| 设置持久化 | `data/settings.json`：`auto_trust`、`downloads_dir`（接收目录）、`theme`（dark/light/system）——全部长期生效 |
| 一键诊断 | 设置页 → “一键导出诊断日志”（先预览后复制；含核心启动失败原因） |

## 5. 构建与测试（全部后台执行，日志写文件）

```powershell
# 一键构建（Windows 桌面 + Android APK → dist/）
powershell -ExecutionPolicy Bypass -File moonletter\tools\build_all.ps1

# Rust 全部测试（17 单元 + 7 e2e，真实 socket）
$env:CARGO_HOME='D:\Lunote 2\.toolchains\cargo-home'
$env:CARGO_TARGET_DIR='D:\Lunote 2\.toolchains\rust-target'
cargo test --manifest-path moonletter\Cargo.toml --workspace --no-fail-fast

# Flutter 静态检查
& 'D:\Lunote 2\.toolchains\flutter\flutter\bin\flutter.bat' analyze

# 集成测试（真实 App；改 UI 后必跑）
# 设备发现 / 返回按钮 / 窗口控制：
cd moonletter\app
& 'D:\Lunote 2\.toolchains\flutter\flutter\bin\flutter.bat' test integration_test\device_discovery_test.dart -d windows
& 'D:\Lunote 2\.toolchains\flutter\flutter\bin\flutter.bat' test integration_test\back_button_test.dart -d windows
& 'D:\Lunote 2\.toolchains\flutter\flutter\bin\flutter.bat' test integration_test\window_controls_test.dart -d windows

# 跨进程网络回归（真实双 CLI 进程）
python tests\regression_cli.py
```

## 6. 发布产物（统一输出目录）

- **`dist\月笺\`**：完整 Windows 桌面目录（双击 `lunote_app.exe` 即用）
- **`dist\月笺.apk`**：Android 安装包
- 每次 `build_all.ps1` 自动同步；历史构建物在 `app\build\` 下（可删，以 dist 为准）

## 7. 大改 UI 的安全守则

1. **只改 `app/lib/src/ui/**`**，不动 `crates/` 与 `app/lib/src/core/`、`app_state.dart` 的方法签名。
2. 数据一律来自 `AppState`（或 `core.call`），**禁止**在 UI 里造数据/假设备。
3. 新页面要刷新数据时，调用已有的 `refreshPeers()` / `refreshConversations()` 等，不要自己再轮询。
4. 事件字段是 snake_case；新增模型字段要两端同步。
5. 对话页被两种方式使用（宽屏内嵌 onBack / 窄屏 push）——改它时两个模式都要测。
6. **配色是双色板（深/浅）**：页面里一律 `final cc = LunoteColors.of(context)` 取当前色，`LunoteColors` 是 ThemeExtension（深/浅两套），不要在页面硬编码颜色或用旧的 `LunoteColors.night` 静态常量。
7. 改完必须：`flutter analyze` + 三个集成测试全过。
8. 怀疑核心问题时先看 `core.log`（核心正常但 UI 空 = UI 链路问题，查 `core_client.dart`；`lunote_create` 启动失败原因也写在这里）。
9. 改前先备份：`D:\Lunote 2\备份\` 下有源码 tar.gz（或随时再打一个）。
10. 构建产物只认 `dist\`，别去 `app\build\...` 深处找。`dist\月笺\lunote_app.exe` 时间戳旧是正常的（功能在 app.so/dll）。

## 8. 已知边界（详见 docs/安全模型.md、交付报告.md）

- 信任：TOFU + 按钮确认；同名同 IP 自动信任默认开（设置页可关）
- 发现信标明文（设备名）；记录元数据不加密、内容加密
- 1:1 会话；同一时刻单发送方向（多文件排队）
- Linux 桌面二进制需 Linux 主机（CI 工作流已提供）

## 9. 接手报告

接替维护者请先读 **`docs/接手报告.md`**（未完成事项、已知问题、验证矩阵、下一步）。
