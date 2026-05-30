# Agent Guidelines

## 基本行为
- 使用**中文**与用户交流
- 遇到需求不明确时，主动提问，不自行假设
- 修改现有代码前，先理解当前实现意图
- 单次变更保持小而聚焦，不将重构混入功能修改

## 项目概述

zero 的 **Rust 工具集合 workspace**。每个工具是一个独立 crate，编译出一个 CLI 二进制，
由 zero 经 `.zero/tools.d/*.toml` 以子命令形式调用。当前成员：

- `crates/github-commit-info`：GitHub 仓库指定时间窗 commit 抓取。
- `crates/hf-watcher`：HuggingFace 按类型 trending 监听（`trending` / `model-card` 两子命令）。

新增工具一律新建 `crates/<tool>`，并遵守下方「工具实现规约」。

## 工具实现规约（所有工具必须遵守）

参照 `github-commit-info` / `hf-watcher` 的既定形态，集成 `custom-utils`：

1. **依赖 custom-utils**：统一在根 `[workspace.dependencies]` 声明，工具 crate 以
   `custom-utils = { workspace = true }` 引入。它提供日志、自更新（updater），以及按需的
   Linux 能力（常驻服务可启用 `daemon-async` / `daemon-sync` 接 systemd）。
2. **日志走 custom-utils**：`main` 入口用
   `custom_utils::logger::logger_feature("<bin>", "<spec>", Info, false).build()` 初始化；
   业务代码一律用 `log::info!/debug!/warn!` 宏，**禁止** `println!` 输出日志。
3. **prod feature 必备**：每个工具 crate 暴露 `prod = ["custom-utils/prod"]`。
   **dev 构建**日志输出到控制台（会占用 stdout，仅供本地调试）；
   **prod 构建**日志写入 `{home}/log/{app}` 文件，**stdout 保持干净**。
   因此部署/发布构建必须带 `--features prod`。
4. **stdout 输出契约**：正常路径仅 `println!` 一行**紧凑 JSON**；业务失败（网络 / 404 /
   解析等可恢复错误）输出 `{error, error_kind}` 且**退出码 0**；仅进程级异常退出码非 0。
   日志绝不写 stdout（见第 2、3 条）。
5. **自更新子命令**：提供 `update`（`custom_utils::updater::UpdateConfig`），指向承载本 workspace
   的 GitHub 仓库；`bin_name` 用各自二进制名。
6. **部署**：开发在 Windows、目标设备是 G10（aarch64 Linux）。用 `deploy-g10.ps1` 在交叉编译
   镜像 `huangjiemin/rust_aarch64-gcc_openssl` 的 Docker 容器内逐 crate
   `cargo build --release --target aarch64-unknown-linux-gnu -p <crate> --features prod`，
   再 scp 到 G10 的 `~/.local/bin/`（与 updater 自更新目标一致）。新增工具时在脚本的 `$Bins`
   列表追加 `(crate, bin)` 一行即可。CI（`.github/workflows/release.yml`）走相同的 per-crate
   prod 构建，推 `v*` tag 产出 `<bin>_<target>[.exe]` 资产供 `update` 子命令自更新。

## 技术栈

| 关注点       | 选型与约束 |
|-------------|-----------|
| 异步运行时   | `tokio`（full features） |
| 日志         | `log` 宏（`info!`、`error!` 等）；初始化用 `custom_utils::logger::logger_feature`；**禁止** `println!` 输出应用日志 |
| HTTP 客户端  | `reqwest` + `rustls-tls`（无 OpenSSL 依赖）；始终用异步 `Client`，**禁止**阻塞 API |
| 错误处理     | `anyhow::Result` + `?` 传播；需上下文时加 `.context("...")` |
| 序列化       | `serde` + `serde_json` |

## 代码结构
- `src/main.rs`：只做运行时启动和日志初始化，保持精简
- `src/lib.rs` 及子模块：承载所有业务逻辑；规模增长时拆分子模块，通过 `lib.rs` 统一导出

## 代码质量

**格式**：遵循 `rustfmt.toml`（120 列，4 空格缩进）和 `clippy.toml` 阈值。

**错误处理**：
- `lib.rs` 及子模块：禁止 `.unwrap()` / `.expect()`，一律用 `?` + `anyhow::Result`
- `main.rs` 和测试代码：允许 `.unwrap()`
- 禁止用 `#[allow(...)]` 压制警告；确有必要时须在注释中说明理由

**依赖管理**：
- 未经用户明确同意，不得添加新依赖
- 引入前评估必要性，优先选维护良好、传递依赖少的 crate
- 不确定选型时，向用户列出候选方案及取舍，不自行决定

## 修复流程
每次代码修改后，必须按以下循环执行，**全部通过才视为完成**：

1. `cargo clippy -- -D warnings`
2. `cargo fmt --check`
3. `cargo test`

若任一步骤失败，继续修复并重新执行完整循环，直到三项全部通过。  
**禁止在循环未完成时停下来，不得以"请你测试一下"结束任务。**

## CI / 发布
- 构建目标：`x86_64-pc-windows-msvc`、`aarch64-unknown-linux-gnu`
- 推送 `v*` 标签触发 Release；推送前本地确认修复流程全部通过
