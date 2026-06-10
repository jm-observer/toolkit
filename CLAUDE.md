# CLAUDE.md

**toolkit 工具中台**（tools-server）：把 ASR / 抖音 / RAG / 长任务等基础能力集中到一个 Cargo
workspace，作为 zero/Agent 生态的统一工具底座。架构目标：
`wechat → Agent(zero) ⇄ llm(GB10) ⇄ tools-server(toolkit-*) ⇄ english`，本仓库即「tools-server」。

> 本仓库由 `github-commit-info` 提级改名而来：原 `github-commit-info` 现降级为众多工具中的一个
> CLI crate。提级规划见 [docs/toolkit-rfc/2026-06-10-toolkit-elevation/plan.md](docs/toolkit-rfc/2026-06-10-toolkit-elevation/plan.md)。

---

## workspace 成员与职责

依赖方向自下而上：`toolkit-core → toolkit-tasks → toolkit-server`；业务 crate（douyin/rag）被 server 装配。

| crate | 职责 |
|---|---|
| `toolkit-core` | 领域类型 + SQLite schema/迁移（`schema.rs` 的 `DDL_V1`）+ URL 模式识别。`open_pool` / `migrate` / `new_task_id` / `now_iso8601`。 |
| `toolkit-tasks` | **通用长任务引擎**：`TaskKind` trait + `Registry` 注册、`submit` 即 spawn、`run_task` 状态机、`store` 持久化到 `tasks` 表。 |
| `toolkit-server` | axum daemon。`bootstrap` 装配 pool/migrate/registry/recovery；`/api/web`、`/api/web/douyin`、`/api/agent`、`/api/browser` 路由 + web 控制台。systemd 安装 / 自更新（`custom-utils` updater）。 |
| `toolkit-desktop` | Tauri 桌面端：抖音 / 同花顺登录窗（headless_chrome/CDP）、msToken 采集、cookie 自动上传 G10。**需 Tauri 工具链**，CI 式环境通常排除。 |
| `douyin` | 抖音 web 工具：a-bogus 签名、creator/works/tags API、下载 + ASR 管线、knowledge md 生成。既是库（被 server 调）也有独立 daemon/CLI。 |
| `rag` | 抖音 knowledge md 的语义检索 → sqlite-vec。CLI `ingest`/`search`，HTTP `serve`。 |
| `github-commit-info` | 独立 CLI：取 GitHub 仓库指定时间范围 commit。 |
| `hf-watcher` | 独立 CLI：HuggingFace trending / model-card 监听。 |

## 常用命令

```bash
# 构建 / 测试（desktop 需 Tauri，CI 式环境排除）
cargo check --workspace
cargo test  --workspace
cargo check --workspace --exclude toolkit-desktop      # 无 Tauri 工具链时
cargo fmt

# 本地起 server（workspace = 所有持久状态的根目录）
cargo run -p toolkit-server -- serve --workspace ./data --bind 127.0.0.1:8788
# 健康检查
curl http://127.0.0.1:8788/api/web/health
```

```powershell
# G10 交叉编译 + 部署（aarch64-linux，Docker 跨编译镜像 → scp 到 G10），见 deploy-g10.ps1
pwsh ./deploy-g10.ps1            # 完整构建并部署
pwsh ./deploy-g10.ps1 -SkipBuild # 仅复制已有产物
```

`deploy-g10.ps1` 的 `$Bins` 列表（crate→bin）控制部署哪些二进制；新增工具时在此追加一行。

## 关键约定

- **TaskKind 注册**：实现 `toolkit_tasks::TaskKind`（关联 `Input`/`Output` + `const KIND` + `async fn run`），
  在 `toolkit-server` 的 `bootstrap()` 里 `registry.register::<T>()`。抖音三种 kind 在
  `crates/toolkit-server/src/douyin/kinds.rs::register_all` 统一注册。`submit()` 校验 kind 后立即
  spawn，返回 `task_id`。
- **SQLite 迁移**：单文件 `<workspace>/toolkit.db`。schema 是整块 `DDL_V1`（`CREATE TABLE IF NOT EXISTS`，
  幂等），版本号写 `meta.schema_version`。改 schema 即改 `toolkit-core/src/schema.rs` 并 bump
  `SCHEMA_VERSION`；当前**没有增量迁移框架**，靠幂等 DDL。
- **长任务状态机**：`queued → running → succeeded/failed`；进程启动时 `recover_interrupted` 把残留的
  `queued/running` 标为 `interrupted`（不自动重跑）。任务体 panic 被 `run_task` 捕获转 `failed`。
  运行中用 `TaskCtx::report_progress(json)` 写 `tasks.progress`。抖音 kind 的形态是「调下游 submit
  → 每 2s 轮询下游状态写进 progress → 终态返回/报错」。
- **输出契约（CLI 工具）**：`douyin` / `hf-watcher` / `github-commit-info` 向 **stdout 输出单行紧凑 JSON**；
  业务失败输出 `{error, error_kind}` 且**退出码 0**（仅进程级异常退出码非 0）。应用日志走
  `custom-utils` logger（prod 落文件，绝不污染 stdout）。
- **workspace 目录布局**（`toolkit-server --workspace` 根）：`toolkit.db`、`douyin/{cookies.json,tasks,transcripts,works}`、
  `downloads/douyin/`、`knowledge/douyin/`、`web/`（静态控制台，缺失则用内嵌最小 HTML）。
- **自更新**：各 bin 的 `REPO_OWNER`/`REPO_NAME` 常量指向 `jm-observer/toolkit`；改名后已统一为 `toolkit`。

## 追踪（trace-hub）

`toolkit-server` 启动时若设了环境变量 `TRACE_HUB_ENDPOINT` 则接入 trace-hub（`custom-utils` 0.15 +
`trace` feature），**未设则完全无副作用**。`toolkit-tasks` 的 runner 用 `SpanScope` 两阶段 API 给每个
任务打 anchor（submit 时 in-flight + 输入摘要）+ 完成 span（成功/失败 + 耗时）。创建任务的 HTTP handler
透传 W3C `traceparent`。详见下方《文档目录》。

## 文档目录（动手前按主题查）

- [docs/toolkit-design.md](docs/toolkit-design.md) — 中台整体设计。
- [docs/douyin-design.md](docs/douyin-design.md) / [docs/douyin-cli.md](docs/douyin-cli.md) — 抖音工具设计与 CLI/HTTP API 参考。
- [docs/rag-service-design.md](docs/rag-service-design.md) — RAG 检索服务设计。
- [docs/toolkit-rfc/2026-06-04-initial-skeleton/data-model.md](docs/toolkit-rfc/2026-06-04-initial-skeleton/data-model.md) — SQLite 数据模型。
- [docs/toolkit-rfc/2026-06-10-toolkit-elevation/plan.md](docs/toolkit-rfc/2026-06-10-toolkit-elevation/plan.md) — 提级为统一工具中台的分阶段规划。
- [docs/retrospective.md](docs/retrospective.md) — 复盘记录。

## 编码约定

- 平台 Windows 11 / PowerShell 优先；提交走 Conventional Commits（中文 message，与既有 git log 一致）。
- 库代码用 `anyhow::Result` + `?` + `.context`；`main.rs`/测试可 `unwrap`。
- 异步上下文禁同步阻塞 I/O；SQL 全参数化。
