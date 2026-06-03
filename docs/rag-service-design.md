# RAG 服务设计

> 时间：创建 2026-06-03 / 最后更新 2026-06-03
> 状态：设计中（实施进行）

## 项目现状

- 抖音知识化主线把每个作品逐条机械录入为 `knowledge/douyin/<抖音号>/transcripts/<aweme_id>.md`（标题 + ASR 文本），由 `crates/douyin` 产出。
- 语义检索（RAG）能力此前写在 **zero 仓**的 `crates/knowledge-rag`（Plan 1–4 完成），但：
  - 从未在 zero 主体接线使用（`app.rs`/`bridge-claw` 零引用，是孤立库代码）；
  - g10 上没有 `[rag]` 配置、没有 `rag.db`、数据未向量化；
  - embedding 服务此前缺失。
- 结论：RAG 实际从未跑起来。本设计把它从 zero 迁出、补齐 embedding 服务、做成 github-commit-info 仓里一个独立服务并接入 Claude Code（经 mcp-server）。

## 整体目标

把 RAG 收敛为 **github-commit-info 仓内一个独立服务**，成为 douyin 知识库的语义检索单一出口：

```
                       ┌─ github-commit-info ────────────────┐
Claude Code            │  crates/rag （binary）               │
   │ MCP/stdio         │   • rag ingest / rag search  (CLI)   │
   ▼                   │   • rag serve                (HTTP)  │
mcp-server  ──http/ssh─►   knowledge-rag 逻辑内化（自包含配置）│
                       │   拥有 rag.db (sqlite-vec)            │
                       └──────────────┬──────────────────────┘
                                      │ POST /v1/embeddings
                                      ▼
                       vLLM(bge-m3, dim=1024) @ g10:8092
```

设计原则：
- **单一 owner**：RAG 代码、`rag.db`、embedding 配置只有一份，谁要用谁当客户端（douyin 同仓直调；zero 将来要用走 HTTP；mcp-server / Claude Code 走 HTTP 或 ssh）。
- **零跨仓耦合**：rag 不依赖 zero 任何 crate，自带配置结构（不再 `use config::...`）。
- **复用 douyin 生态约定**：clap 子命令、axum serve、stdout 一行 JSON、绝对路径由调用方传入。

## crate 结构

```
crates/rag/
├── Cargo.toml
└── src/
    ├── main.rs            # clap 入口：ingest / search / serve
    ├── lib.rs             # 重导出
    ├── config.rs          # 自包含 RagConfig（替代 zero config crate 耦合）
    ├── types.rs           # IngestItem / SearchHit / SearchQuery（移植）
    ├── embedding.rs       # EmbeddingProvider trait（移植）
    ├── embedding_http.rs  # OpenAiCompatEmbedding（移植，改读 RagConfig）
    ├── store.rs           # VectorStore trait + StoredChunk（移植）
    ├── store_sqlite.rs    # SqliteVecStore（移植，改读 RagConfig）
    ├── normalize.rs       # 文本归一/切块（移植，零改动）
    ├── service.rs         # KnowledgeRagService 编排（移植，零改动）
    ├── ingest.rs          # 扫 douyin 知识包目录 → service.ingest
    └── serve.rs           # axum HTTP
```

移植自 zero `crates/knowledge-rag` 的文件保持逻辑不变；唯一改动是把 `embedding_http.rs`/`store_sqlite.rs` 里对 `config::RagEmbeddingProviderSection` / `config::RagStoreSection` 的引用换成本 crate `config.rs` 自带的等价结构。

## 配置（自包含）

`RagConfig` 从 **JSON** 文件加载（绝对路径由调用方传 `--config`；JSON 与 douyin
生态约定一致，避免新引入 `toml` 依赖），结构等价于原 zero `[rag]` 段，去掉
`enabled`（独立服务无需开关）。样例见 `crates/rag/deploy/rag.config.json`：

```json
{
  "embedding": {
    "endpoint": "http://127.0.0.1:8092/v1/embeddings",
    "model": "BAAI/bge-m3",
    "api_key_env": null,
    "dim": 1024,
    "timeout_secs": 30
  },
  "store": { "db_path": "rag.db" },
  "chunk_max_chars": 800,
  "chunk_overlap_chars": 80
}
```
（`dim` 必须与 bge-m3 一致并写进 chunks_vec schema；`db_path` 相对 workspace_root；
`api_key_env` 为 null/空表示无鉴权。）

- `workspace_root` 解析沿用 douyin 约定：`--workspace` 显式优先，否则 `ZERO_WORKSPACE` 环境变量，再否则 `$HOME/.config/zero`。
- 维度校验：`SqliteVecStore::open` 在已有 `rag.db` 时校验 `chunks_vec` 维度与 `dim` 一致，不一致报错引导删库重建。

## ingest 源与映射

| 项 | 取值 |
|----|------|
| 扫描根 | `<workspace>/knowledge/douyin/<抖音号>/transcripts/*.md` |
| `namespace` | `douyin` |
| `external_id` | 文件名去扩展（即 `aweme_id`） |
| `text` | 整个 `.md` 文件内容（service 内部 normalize + chunk） |
| `metadata` | `{ author_id: <抖音号>, source_path: <相对路径>, mtime_secs: <修改时间> }` |

- 备用源：`<workspace>/douyin/transcripts/<aweme_id>.json`（原始 ASR `{aweme_id,text}`），首版不用，留作降级。
- 增量：`ingest` 子命令支持 `--since-mtime` 或全量重扫（upsert 幂等，重复 ingest 安全）。首版做全量重扫，后续可加 mtime 增量表。

## 接口

### CLI（stdout 一行紧凑 JSON，退出码恒 0）

| 命令 | 参数 | 输出 |
|------|------|------|
| `rag ingest` | `--config <p>` `--workspace <p>` `[--namespace douyin]` | `{"ingested":N,"skipped":M,"failed":K}` |
| `rag search` | `--config <p>` `--workspace <p>` `--query <q>` `[--namespace douyin]` `[--top-k 5]` | `{"hits":[{external_id,chunk_index,text,score,metadata}]}` |
| `rag serve`  | `--config <p>` `--workspace <p>` `[--bind 127.0.0.1:8788]` | （daemon） |

业务失败输出 `{"error":...,"error_kind":...}`，与 douyin 一致。

### HTTP（axum，默认 `127.0.0.1:8788`）

| 路由 | 方法 | body | 返回 |
|------|------|------|------|
| `/healthz` | GET | — | `{"status":"ok"}` |
| `/v1/search` | POST | `{namespace?,query,top_k?}` | `{"hits":[...]}` |
| `/v1/ingest` | POST | `{namespace?}` | `{"ingested":N,...}` |

端口避开已占用的 8787(douyin)/8090/8091/8095。

## mcp-server 接入

首选 `http` 工具类型指向 rag serve；**注意 mcp-server 的 SSRF 防护会拦私有/环回地址**，g10 是 `192.168.0.68`（私有），需先验证 mcp-server 是否放行：

- 若放行 → `tools.d/rag.toml` 用 `type="http"` 指 `http://192.168.0.68:8788/v1/search`。
- 若被拦 → 退回 `type="command"`，跑 `ssh fengqi@192.168.0.68 ~/.local/bin/rag search ...`（query 经 stdin/base64 传，避免远端 shell 二次切词）。

Claude Code 侧用 `.mcp.json` 以 stdio 启动 mcp-server。

## 部署（g10 / aarch64）

- embedding：vLLM 跑 `BAAI/bge-m3 --task embed`，`--gpu-memory-utilization 0.08`（GB10 统一内存紧，gemma4 已占大头），暴露 `:8092/v1/embeddings`，dim=1024。**由用户部署。**
- rag binary：沿用 `deploy-g10.ps1`（Docker 交叉编译 aarch64 + scp 到 `~/.local/bin`）。
- 首次：g10 上 `rag ingest` 灌库 → `rag search` 验证召回 → 起 `rag serve`（可 systemd）。

## zero 侧清理

knowledge-rag 在 zero 是孤立代码，迁出后删除：

- 删 `crates/knowledge-rag/`（workspace member）。
- 删 `crates/config/src/rag.rs`；`crates/config/src/lib.rs` 移除：`mod rag` / `pub use rag::*` / `ZeroConfig.rag` 字段 / default 初始化 / `hot_field_classification` 的 `("rag", ...)` / 2 个 rag 相关测试。
- `docs/rfc/2026-06-02-rag/` 标记被本设计取代并 `git mv` 至 `docs/old/`，同步 `docs/README.md`。
- 跑 `cargo make check` 确认通过。

## Plan 拆分与状态

| Plan | 内容 | 状态 |
|------|------|------|
| P1 | crates/rag 脚手架 + 移植 knowledge-rag + 自包含 config.rs | ✅ 完成 |
| P2 | rag CLI（ingest 扫 douyin 知识包 + search） | ✅ 完成 |
| P3 | rag serve（axum HTTP，GET+POST /v1/search、/v1/ingest、/healthz） | ✅ 完成 |
| P4 | zero 侧清理 knowledge-rag（含 custom-utils 升 0.15、文档归档） | ✅ 完成 |
| P5 | mcp-server tools.d + Claude Code .mcp.json + 端到端验证 | 配置/接线产物就绪（`deploy/`），端到端验证待部署 |

> 校验状态：rag crate `cargo check`/`clippy -D warnings`/`fmt`/17 单测全过；zero
> `cargo check --workspace`/`clippy --workspace`/`fmt`/config 测试全过。

## 风险与待定项

- **embedding 服务可用性**：依赖用户在 g10 起 bge-m3；未就绪前 ingest/search 无法真跑，但代码与单测可先完成。
- **mcp-server SSRF 拦私有 IP**：接入方式二选一，需实测确认。
- **GB10 内存紧张**：bge-m3 与现有 gemma4/ASR/TTS 抢统一内存，需压低 vLLM 占用并观察。
- **ingest 源格式**：当前吃知识包 `.md`；若后续 douyin 改用别的产物布局需同步调整扫描根。
