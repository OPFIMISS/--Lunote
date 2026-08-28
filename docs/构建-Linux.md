# Linux 构建说明

月笺 Lunote 的 Linux 支持分两部分：

## 1. Rust 核心（Linux 原生）

核心与 CLI 是纯 Rust + tokio，Linux 上直接构建：

```bash
cargo build --release -p lunote-cli -p lunote-bridge
```

产物：`target/release/lunote-cli`、`target/release/liblunote_bridge.so`。
无 Linux 特有依赖（网络层用 std/tokio；文件选择走 Flutter 侧插件）。

## 2. Flutter 桌面 UI（Linux）

`app/linux/` 已包含完整 runner（由 `flutter create --platforms linux` 生成），
在 Linux 主机上：

```bash
cd app
flutter pub get
flutter build linux --release
cp ../target/release/liblunote_bridge.so build/linux/x64/release/bundle/lib/
```

产物：`build/linux/x64/release/bundle/lunote_app`（可执行）及 `lib/`。

## 3. 本仓库验证状态（如实）

- 当前开发机是 Windows，**无法在本机生成 Linux 二进制**（Flutter Linux 需要
  Linux 工具链），因此：
  - Linux 构建配置已提供（`app/linux/` + CI）；
  - GitHub Actions 工作流 `.github/workflows/build.yml` 会在 ubuntu 上产出
    Linux 交付物；
  - Rust 核心在 Windows 上交叉编译 Linux 目标未配置（需要 zig/linux 工具链），
    列为后续改进。

## 4. 运行时说明（Linux）

- 首次运行需放行 UDP 45454（发现）与 TCP 45455（会话）入站（firewalld/ufw）；
- 托盘依赖系统托盘实现（GNOME 需 AppIndicator 扩展）；
- 数据目录：`~/.local/share/com.lunote.lunote_app/data`。
