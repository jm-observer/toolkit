# Plan 1：toolkit-core schema + toolkit-tasks 引擎 + 最小 toolkit-server

## 前置依赖

无。

## 任务目标

1. 新增 **3 个** crate（`toolkit-core` / `toolkit-tasks` / `toolkit-server`）并接入 workspace
2. `toolkit-core` 提供 SQLite 初始化 + schema v1 全部表（DDL 见 [data-model.md](data-model.md)） + URL 模式识别
3. `toolkit-tasks` 提供 `TaskKind` trait + `submit/status/list` 三个 API + 进程内调度器 + 持久化
4. `toolkit-server` 启动后：
   - 加载/迁移 SQLite
   - 提供 `/api/web/health` GET、`/api/web/tasks` GET/POST、`/api/web/tasks/{id}` GET
   - 提供 `/api/browser/hello` / `/api/browser/url` / `/api/browser/cookie` POST endpoints（按 [extension-contract.md](extension-contract.md)）
   - 提供 `/api/agent/health` GET（占位，确认 namespace 分流跑通）
   - 静态托管最小 dashboard HTML（仅最小验证）
5. 注册一个内置测试 task kind：`Echo`（输入 `{message, delay_ms}`，sleep 后返回同样消息），用于端到端验证 submit→status→succeeded

## 执行范围

**必须修改**：
- `Cargo.toml`（workspace）：添加 3 个新 members + 新增 workspace 依赖（`r2d2`、`r2d2_sqlite`、`tower-http`、`uuid`、`regex`）
- 新建 `crates/toolkit-core/`、`crates/toolkit-tasks/`、`crates/toolkit-server/`

**允许修改**：无

**禁止修改**：
- `crates/douyin/`（一行不动）
- `crates/rag/`
- `crates/github-commit-info/`
- `crates/hf-watcher/`

## Agent 执行步骤

### 步骤 1：workspace 接入

1. 编辑根 `Cargo.toml`：
   - `members` 添加 3 个新 crate
   - `[workspace.dependencies]` 新增：
     ```toml
     r2d2 = "0.8"
     r2d2_sqlite = "0.24"
     tower-http = { version = "0.6", features = ["fs", "trace", "cors"] }
     uuid = { version = "1", features = ["v4"] }
     regex = "1"
     ```
2. 在每个新 crate 的 `Cargo.toml` 用 `{ workspace = true }` 引用

### 步骤 2：`toolkit-core` 实现

文件结构：
```
crates/toolkit-core/
├── Cargo.toml
└── src/
    ├── lib.rs           # re-exports
    ├── db.rs            # SqlitePool 封装（r2d2_sqlite）
    ├── migrations.rs    # schema_version + migrate(conn) → Result<()>
    ├── schema.rs        # 嵌入 v1 DDL 常量（按 data-model.md 全文）
    ├── models.rs        # Creator/Work/Task/Cookie/BrowserSession 结构（serde）
    ├── ids.rs           # task_id 生成：format!("tk_{}", short_uuid())
    └── url_match.rs     # 抖音 URL 模式识别（按 extension-contract §五）
```

关键签名：
```rust
pub fn open_pool(path: &Path) -> Result<SqlitePool>;
pub fn migrate(pool: &SqlitePool) -> Result<()>;  // 启动时调一次

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UrlMatch {
    CreatorHome { sec_uid: String },
    CreatorHomeShort { short_code: String },
    Work { aweme_id: String },
    Search,
    None,
}
pub fn classify_url(url: &str) -> UrlMatch;
```

### 步骤 3：`toolkit-tasks` 实现

文件结构：
```
crates/toolkit-tasks/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── kind.rs          # TaskKind trait + Registry
    ├── runner.rs        # 内部调度：spawn tokio::task 跑 K::run，回写 DB
    ├── store.rs         # tasks 表 CRUD（依赖 toolkit-core）
    ├── api.rs           # submit / status / list
    └── echo.rs          # 内置 EchoTask（验证用）
```

`TaskKind` trait：
```rust
#[async_trait]
pub trait TaskKind: 'static + Send + Sync {
    type Input: Serialize + DeserializeOwned + Send;
    type Output: Serialize + DeserializeOwned + Send;
    const KIND: &'static str;
    async fn run(input: Self::Input, ctx: TaskCtx) -> Result<Self::Output>;
}

pub struct TaskCtx {
    pub task_id: String,
    pub pool: SqlitePool,
    pub report_progress: Arc<dyn Fn(serde_json::Value) + Send + Sync>,
}
```

Registry：
```rust
pub struct Registry { /* HashMap<&'static str, Arc<dyn ErasedKind>> */ }
impl Registry {
    pub fn register<K: TaskKind>(&mut self);
    pub fn submit(&self, kind: &str, input_json: Value, pool: &SqlitePool) -> Result<String>;
}
```

调度器：每个 submit 即 `tokio::spawn`，task 内部捕获 panic → state=failed；
**进程启动时**扫描 `state IN ('queued','running')` 的任务，全部标记 `interrupted`（不自动重跑）。

### 步骤 4：`toolkit-server` 实现

文件结构：
```
crates/toolkit-server/
├── Cargo.toml
└── src/
    ├── main.rs          # 启动入口
    ├── config.rs        # 读 env / CLI args（bind addr / db path / data dir）
    ├── state.rs         # AppState { pool, registry, ... }
    ├── static_assets.rs # 嵌入最小 dashboard.html
    └── routes/
        ├── mod.rs
        ├── web.rs       # /api/web/* 路由集合
        ├── agent.rs     # /api/agent/* 路由集合（仅 health 占位）
        └── browser.rs   # /api/browser/{hello,url,cookie} HTTP endpoints
```

路由清单（Plan 1）：

| 方法 | 路径 | 处理 |
|---|---|---|
| GET | `/api/web/health` | 返回 `{ status: "ok", version, db_path }` |
| POST | `/api/web/tasks` | body `{ kind, input }` → 调 Registry.submit → 返回 `{ task_id }` |
| GET | `/api/web/tasks` | query: `kind?`/`state?` → 列表 |
| GET | `/api/web/tasks/{id}` | 返回 tasks 表整行 |
| GET | `/api/agent/health` | 占位，返回同 web/health |
| POST | `/api/browser/hello` | upsert browser_sessions，回 `{server_version, accepted_at}` |
| POST | `/api/browser/url` | 更新 current_url，回 `{matched: <UrlMatch tag>}` |
| POST | `/api/browser/cookie` | upsert cookies 单行，回 `{accepted, fields_count, has_required}` |
| GET | `/` | 静态最小 dashboard |

启动顺序：
1. 解析 config
2. open_pool + migrate
3. 注册 EchoTask
4. **进程崩溃恢复**：把 queued/running 任务标记 interrupted
5. axum listen + serve

CLI / env：
- `--bind 0.0.0.0:8788`（默认）
- `--data-dir ./data`（SQLite 落 `<data-dir>/toolkit.db`）
- 兼容 `TOOLKIT_BIND` / `TOOLKIT_DATA_DIR` 环境变量

CORS：`tower-http::cors::CorsLayer` 允许 `chrome-extension://*` + 任何 Origin（本机内网），覆盖 `/api/browser/*` 路由。

## 目标数据结构 / 接口契约

### TaskStatus DTO

```rust
#[derive(Serialize, Deserialize)]
pub struct TaskStatusDto {
    pub task_id: String,
    pub kind: String,
    pub state: String,          // queued/running/succeeded/failed/cancelled/interrupted
    pub progress: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}
```

### POST /api/web/tasks 请求

```json
{
  "kind": "echo",
  "input": {"message": "hi", "delay_ms": 1000},
  "callback_url": null
}
```

返回：
```json
{ "task_id": "tk_a1b2c3d4e5f6g7" }
```

## 行为规则

| 输入 | 输出 / 状态变化 |
|---|---|
| POST tasks，kind 未注册 | 400 `{error: "unknown kind: <kind>"}`，无 DB 写入 |
| POST tasks，input 反序列化失败 | 400 `{error: "invalid input: ..."}`，无 DB 写入 |
| POST tasks 成功 | tasks 表插入 `state=queued, created_at=now`，立即 spawn → `started_at=now, state=running` |
| TaskKind::run 返回 Ok | `state=succeeded, output=<json>, finished_at=now` |
| TaskKind::run 返回 Err | `state=failed, error=<string>, finished_at=now` |
| TaskKind::run panic | 同 failed，error=`task panicked: <info>` |
| GET tasks/{id}，id 不存在 | 404 |
| POST browser/url，URL 不命中模式 | 200 `{matched: null}`，仍更新 last_seen / current_url |
| POST 任何 browser endpoint，JSON 解析失败 | 400 |
| 进程启动时 state=queued/running 的旧任务 | 改写为 `state=interrupted, finished_at=now, error="process restart"` |

## 禁止事项

- **不**实现任何抖音业务路由（creator/works/download/transcribe/kb_publish）——Plan 2 才做
- **不**改动 `crates/douyin/`、`crates/rag/`
- **不**在 Plan 1 引入前端构建链（Svelte 是 Plan 3）；前端只用最小内嵌 HTML
- **不**在 toolkit-server 调用 douyin/rag 的业务函数——保持骨架纯净
- **不**实现扩展本体（manifest.json + background.js）——Plan 4 才做；本 Plan 仅 server 侧 HTTP endpoint
- **不**在任务运行中读写 `creators` / `works` / `cookies` 表（只 Echo 测试 task）
- **不**实现真实鉴权——按 contract §七，首版无鉴权

## 测试要求

### toolkit-core
- `tests/migrate.rs`：在临时目录建库、跑 `migrate()`、断言 5 张表都存在 + schema_version=1
- `tests/url_match.rs`：覆盖 §五 表里每条 URL 模式各一例 + 一条不命中

### toolkit-tasks
- `tests/echo_roundtrip.rs`：注册 Echo、submit、轮询 status 至 terminal、断言 output 正确
- `tests/restart_recovery.rs`：手工插一条 `state=running` 任务、调 recovery 函数、断言被改为 `interrupted`

### toolkit-server
- `tests/http_health.rs`：起 server（pick random port）、GET /api/web/health 返回 200
- `tests/http_task_lifecycle.rs`：POST echo task → 轮询 GET tasks/{id} 至 succeeded
- `tests/http_browser_endpoints.rs`：POST hello / url / cookie 三个 endpoint，断言响应字段 + DB 副作用

验证命令（在仓根）：
```
cargo test -p toolkit-core
cargo test -p toolkit-tasks
cargo test -p toolkit-server
cargo clippy --workspace -- -D warnings
cargo fmt --check --all
```

## 完成条件（checklist）

- [x] workspace 含 3 个新 crate，`cargo build --workspace` 通过
- [x] `toolkit-server` 启动后 GET /api/web/health 200
- [x] POST /api/web/tasks 提交 Echo 任务，几秒后 GET 看到 `state=succeeded`
- [x] POST /api/browser/hello / url / cookie 三个 endpoint 各返回 200 + DB 副作用正确
- [x] 进程 kill -9 后重启，先前 running 任务已变 interrupted
- [x] `cargo clippy --workspace -- -D warnings` 通过
- [x] `cargo fmt --check --all` 通过
- [x] `cargo test --workspace` 通过
- [x] 现有 douyin/rag/github-commit-info/hf-watcher 测试全部仍通过（未被影响）
