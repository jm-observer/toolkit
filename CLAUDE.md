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
| `toolkit-server` | axum daemon。`bootstrap` 装配 pool/migrate/registry/recovery；`/api/web`、`/api/web/audio`（TTS 代理）、`/api/web/douyin`、`/api/agent`、`/api/browser` 路由 + web 控制台。systemd 安装 / 自更新（`custom-utils` updater）。 |
| `toolkit-desktop` | Tauri 桌面端：抖音 / 同花顺登录窗（headless_chrome/CDP）、msToken 采集、cookie 自动上传 G10。**需 Tauri 工具链**，CI 式环境通常排除。 |
| `asr-server` | 独立 OpenAI 兼容 ASR HTTP 服务（sherpa-onnx）：`/healthz`、`/v1/models`、`/v1/audio/transcriptions`（multipart）、`/v1/audio/transcriptions/from-source`（JSON file:///http）。由 streaming-speech 整 crate 迁入（Phase 1）。模型走 `<workspace>/models/`，不入仓库。douyin process 调它的 from-source 端点。 |
| `douyin` | 抖音 web 工具：a-bogus 签名、creator/works/tags API、下载 + ASR 管线、LLM 整理（`refine`）、knowledge md 生成。既是库（被 server 调）也有独立 daemon/CLI。 |
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
  在 `toolkit-server` 的 `bootstrap()` 里 `registry.register::<T>()`。抖音 kind 在
  `crates/toolkit-server/src/douyin/kinds.rs::register_all` 统一注册：`douyin_download` /
  `douyin_transcribe` / `douyin_list_works`（文件状态轮询型）+ `douyin_text_refine`（LLM 整理，
  进程内逐条调）+ `douyin_pipeline`（整链编排）。`submit()` 校验 kind 后立即 spawn，返回 `task_id`。
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
- **workspace 目录布局**（`toolkit-server --workspace` 根）：`toolkit.db`、`douyin/{cookies.json,tasks,transcripts,refined,works}`、
  `downloads/douyin/`、`knowledge/douyin/`、`web/`（静态控制台，缺失则用内嵌最小 HTML）。
  `douyin/refined/<aweme_id>.json` = LLM 整理稿（与 ASR 原文 `transcripts/<aweme_id>.json` 并列）。
- **自更新**：各 bin 的 `REPO_OWNER`/`REPO_NAME` 常量指向 `jm-observer/toolkit`；改名后已统一为 `toolkit`。

## 追踪（trace-hub）

`toolkit-server` 启动时若设了环境变量 `TRACE_HUB_ENDPOINT` 则接入 trace-hub（`custom-utils` 0.15 +
`trace` feature），**未设则完全无副作用**。`toolkit-tasks` 的 runner 用 `SpanScope` 两阶段 API 给每个
任务打 anchor（submit 时 in-flight + 输入摘要）+ 完成 span（成功/失败 + 耗时）。创建任务的 HTTP handler
透传 W3C `traceparent`。详见下方《文档目录》。

## 语音底座（ASR / TTS，Phase 1）

- **ASR**：`asr-server` crate（见成员表）。同机部署，douyin process 的 `asr_url` 默认
  指向 `http://127.0.0.1:8091/.../from-source`。模型放 `<workspace>/models/sherpa-sense-voice/`，
  不入仓库。`silero_vad.onnx` 随 crate 提交。
- **TTS 代理**：`toolkit-server` 的 `/api/web/audio/tts`（POST，转发请求体到上游
  CosyVoice2 `POST /tts`，回传 WAV bytes）与 `/api/web/audio/voices`（GET，代理 `/voices`）。
  上游地址由环境变量 **`TTS_BASE_URL`**（如 `http://127.0.0.1:8095`）配置；**未配置时
  两端口返回 503** 并提示。TTS 生成可能 10s+，代理超时 180s。调用上有 `SpanScope`
  两阶段 trace（`tts_proxy` / `tts_voices` span；trace 未启用时 no-op）。本阶段只代理，
  不落盘 / 不任务化（落盘任务化是 Phase 3 AudioForge）。
- **编排**：`deploy/asr-tts/`（compose + README）——ASR + TTS 与 toolkit-server 同机
  部署的最小可用编排。

## 抖音知识管线（流 A，Phase 2）

补齐了 plan 流 A 的「LLM 整理文本」与「整链编排」两块缺口：

- **TextRefine**（`douyin_text_refine` kind / `POST /api/web/douyin/refine`）：读 ASR 原文
  （`douyin/transcripts/<id>.json`）→ 调 GB10 vLLM（OpenAI 兼容 chat completions）纠错/去口语
  水词/分段/小结 → 落整理稿 `douyin/refined/<id>.json`（带 `model` / `prompt_version` /
  `prompt_hash` / `refined_at`）。输入显式 `aweme_ids` 或留空整理「全部已转写未整理」。单条失败
  重试 3 次（指数退避），最终失败进 output 的 `failures[]`，不拖垮整批。**幂等**：已整理跳过。
- **整理稿进 RAG**：`kb_publish` 把整理稿写进 knowledge md 的 `## 整理稿（LLM）` 段（置于 ASR 原文
  之前，rag 优先索引整理后的可读文本），frontmatter 记 `has_refined` + refined 元信息；原文栏保留。
- **CreatorPipeline**（`douyin_pipeline` kind / `POST /api/web/douyin/pipeline`）：输入
  `handle`（unique_id/URL）+ 可选 `tags` 筛选 + `stages` 开关，串联
  `sync_works(可选)→download→transcribe(ASR)→refine→kb_publish→rag_ingest`。进度聚合写
  `progress.{stage,stage_index,stage_total,stage_progress}`。任一环节失败 → 任务 failed，已完成成果
  保留（各下游任务自身幂等，重跑跳过已完成 item）。`rag_ingest` 通过 spawn `rag` 二进制完成
  （需 `rag_config` JSON 路径；rag 定位优先 `RAG_BIN` 否则同目录 `rag`）。
- **LLM 配置（环境变量）**：`LLM_BASE_URL`（OpenAI 兼容 base，如 `http://gb10:8000/v1`，必填）、
  `LLM_MODEL`（必填）、`LLM_API_KEY`（可选）。**未配置时 refine / 含 refine 的 pipeline 提交后
  立即 failed** 并说明缺哪个变量（不空跑下载/ASR）。
- **整理 prompt 管理**：prompt 文本 = `crates/douyin/src/refine_prompt.md`（`{TRANSCRIPT}` 占位符，
  随 crate 编译）；改文案后 bump `refine.rs::PROMPT_VERSION`。每条整理稿记 `prompt_hash`（sm3 短哈希），
  prompt 变了哈希就变，可识别旧产物、删 `refined/` 后重跑对比。
- **端到端验收**：见 [docs/runbook-pipeline-e2e.md](docs/runbook-pipeline-e2e.md)。

## 文档目录（动手前按主题查）

- [docs/toolkit-design.md](docs/toolkit-design.md) — 中台整体设计。
- [docs/douyin-design.md](docs/douyin-design.md) / [docs/douyin-cli.md](docs/douyin-cli.md) — 抖音工具设计与 CLI/HTTP API 参考。
- [docs/rag-service-design.md](docs/rag-service-design.md) — RAG 检索服务设计。
- [docs/runbook-pipeline-e2e.md](docs/runbook-pipeline-e2e.md) — 抖音知识管线端到端验收 runbook（Phase 2）。
- [docs/toolkit-rfc/2026-06-04-initial-skeleton/data-model.md](docs/toolkit-rfc/2026-06-04-initial-skeleton/data-model.md) — SQLite 数据模型。
- [docs/toolkit-rfc/2026-06-10-toolkit-elevation/plan.md](docs/toolkit-rfc/2026-06-10-toolkit-elevation/plan.md) — 提级为统一工具中台的分阶段规划。
- [docs/retrospective.md](docs/retrospective.md) — 复盘记录。

## 编码约定

- 平台 Windows 11 / PowerShell 优先；提交走 Conventional Commits（中文 message，与既有 git log 一致）。
- 库代码用 `anyhow::Result` + `?` + `.context`；`main.rs`/测试可 `unwrap`。
- 异步上下文禁同步阻塞 I/O；SQL 全参数化。
