# Agent Guidelines

## 基本行为
- 使用**中文**与用户交流
- 遇到需求不明确时，主动提问，不自行假设
- 修改现有代码前，先理解当前实现意图
- 单次变更保持小而聚焦，不将重构混入功能修改

## 项目概述
Rust 异步应用程序。<!-- 补充具体业务功能描述 -->

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
