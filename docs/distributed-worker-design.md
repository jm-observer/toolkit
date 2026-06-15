# 分布式 Worker 设计

> 把爬虫类长任务（抖音 list_works / detail / download / ASR 上传等）从单机 `toolkit-server` 中卸载到 N 台**独立出口 IP** 的 worker 节点，规避 IP 维度的 shadow-throttle，并提升整体吞吐。本文档定义节点角色、通信协议、调度模型、自更新通道与运维边界。

## 1. 背景与目标

- 现状：所有抖音 / THS 业务在 `toolkit-server` 进程内执行,出口 IP 单一。抖音 `list_works` 已被实验坐实为 IP 维度 shadow-throttle(见记忆 [project_douyin_v2_ip_root_cause])。
- 目标:
  1. **匿名可爬的页面**(THS 板块成分股前几页、douyin 不依赖 cookie 的接口若有)横向扩展到多 IP,提升吞吐。
  2. 主控不出网爬数据,**只调度 + 收尾**;worker 节点可在 NAT 后、家宽带、云上独立 IP 等任意拓扑。
  3. 新增 worker 节点 ≤ 一行 install 命令;后续升级、配置变更**无人值守**。
- 非目标:
  - 不替换 `toolkit-tasks` 现有任务引擎,**扩展(父任务恢复 + cancel)而非重写**(详见 §0.1)。
  - 不引入消息队列 / 对象存储等新基础设施(首阶段)。
  - 不做跨地域容灾。
- **首个落地 kind = THS 板块成分股翻页**,不是抖音。理由:
  - THS 前几页匿名可爬,worker 多 IP 收益直接,无 cookie 协调开销;
  - 抖音 cookie 绑账号,**多 IP 共享同一 cookie 反而可能触发风控**,sticky affinity 只解决「同 creator 固定到一台」,没解决「账号池横向扩张」;
  - 抖音类 job **默认不做横向扩展**,只有明确不依赖 cookie 的接口才下放 worker;其余继续在 controller / 固定单 worker 上跑,与现状等价。

## 0. 阶段 0:协议 / 存储最小修正(后续阶段的前置)

代码复核暴露出几个**与现有代码不兼容**的点,必须先把这些坑填上,后面阶段 1 才能落得动。每一条都对应代码里已经存在的具体形态,不是空想。

### 0.1 父任务的恢复 / 收尾模型

**问题**:`toolkit-tasks` 现状是 `submit` 即 `tokio::spawn`(`runner.rs::run_task`),进程重启时 `recover_interrupted` 把残留 `queued/running` 直接标 `interrupted`,**没有恢复任何聚合循环**。若父任务在 `run()` 里轮询 `jobs` 表,controller 重启后子 job 在 worker 上还在跑,但**父任务没人接收尾结果**,这是死锁雏形。也没有取消接口。

**最小修正**:把"派子 job + 轮询聚合"做成 **可恢复的状态机**,而不是父进程内存里的 future。需要给 `TaskKind` 显式增加「派发完毕,等外部收尾」的合法返回值——否则现有 `run_task` 在 `Ok(_)` 之后直接 `mark_succeeded`,跟分布式语义对不上。

- **TaskKind 返回值扩展(泛型,保留 typed output)**:

  ```rust
  pub enum RunOutcome<T> {
      Done(T),             // 现状语义:runner 序列化 T 后调 mark_succeeded
      PendingExternal,     // 新增:派发完毕,future 退出但任务保持 running,
                           //       由 aggregator 负责终态(见下)
  }
  // TaskKind::run() 签名:
  //   旧: async fn run(&self, input: Self::Input, ctx: TaskCtx) -> Result<Self::Output>
  //   新: async fn run(&self, input: Self::Input, ctx: TaskCtx) -> Result<RunOutcome<Self::Output>>
  ```

  关联类型 `Output` 保留,序列化仍由 `ErasedKind` wrapper 统一负责(现状),业务代码继续返回强类型。**非分布式 kind 改造 = 把 `Ok(out)` 改成 `Ok(RunOutcome::Done(out))`,一行 sed 改完**。

  `runner::run_task` 处理三分支:
  - `Ok(Done(out))` → 现状路径,`mark_succeeded`。
  - `Ok(PendingExternal)` → **不**调 `mark_succeeded`,emit `dispatched` 中间 span,task 保持 `state='running'`,等 aggregator 接管。
  - `Err(_)` / panic → 现状路径,`mark_failed`(派发段自身失败立刻终结,不能进 pending)。

  分布式父任务通常没有有意义的 `Done` 路径(终态在 aggregator),`Output` 可以是单元类型或包装类型,trait 不强制使用。

- **分布式父任务 `run()` 的写法**:

  ```rust
  type Input = SyncSectorsInput;
  type Output = ();   // 终态由 aggregator 写,这里不返回 Done

  async fn run(&self, input: Self::Input, ctx: TaskCtx)
      -> Result<RunOutcome<Self::Output>>
  {
      dispatch_jobs(&ctx.pool, &ctx.task_id, &plan(input))?;   // INSERT OR IGNORE
      ctx.report_progress(json!({ "phase": "dispatched", "total": plan_len }))?;
      Ok(RunOutcome::PendingExternal)
  }
  ```

- **Aggregator(controller 进程内独立 tick,每 5s)**:扫 `tasks WHERE state='running' AND kind ∈ distributed_kinds`,按 `parent_task_id` 聚合 `jobs` 表的终态:
  - 全部子 job 进终态(succeeded/failed/cancelled) → 调 `mark_succeeded({ ok: [...], failures: [...] })` 或 `mark_failed`(按业务策略,例如「任意子 job failed → 父 failed」或「失败比例阈值」)。
  - 还有未终态 → 更新父 `progress.{done, total, failures}`,继续等。
  - **Aggregator 是真相源**,不依赖父进程内任何内存状态。controller 重启后 tick 重新捞起所有 `running` 父任务,无缝接管。

- **`recover_interrupted` 行为微调**:对 `kind ∈ distributed_kinds` 的 `running` 任务**不标 interrupted**(aggregator 会接管);仍把分布式父任务里 `queued` 状态的标 interrupted(派发段还没跑完就重启,需要重新提交)。`distributed_kinds` 在 registry 注册时通过 `register_distributed::<T>()` 声明。

- **取消通路**:`POST /api/web/tasks/{id}/cancel` 在 `tasks` 表加 `cancel_requested=1`;aggregator tick 时把同 parent 的 `queued` jobs 直接 `state='cancelled', cancelled_at=now`,`leased` jobs `cancel_requested=1`;`/jobs/progress` 响应 `cancel: true` 让 worker 优雅停;worker `complete(error_kind='cancelled')` → `state='cancelled'`;aggregator 看到全部子 job 终态后把父 `mark_failed("cancelled")`。
- **取消 + 断线竞态(修正 P1-a)**:若 worker 收到 cancel 后立即断线没 complete,reaper 看到该 lease 过期**必须先检查 `cancel_requested`**——为 1 时直接 `state='cancelled', cancelled_at=now`,**不回队**(否则取消任务会被重新 lease 复活)。下面 §4.4 reaper 描述与此一致。

- **向后兼容**:现有所有非分布式 kind 返回 `RunOutcome::Done(v)`,runner 行为不变。`TaskKind::run` 签名变化是一次 trait breaking change,在 `toolkit-tasks` 一次性扫所有 impl 即可,无运行时风险。

### 0.2 Worker 调用 douyin / process 库的形态

**问题**:`douyin::download::submit` / `list_works_task::submit` / `process::submit` 都走 `std::env::current_exe()` 拉**隐藏子命令**(`download-worker` / `list-works-worker` / `process-worker`)。toolkit-worker 进程 exe 名不一样,直接调这些 `submit()` 会去 spawn `toolkit-worker download-worker ...` 失败,或者绕开 lease 生命周期。

**最小修正**:**worker 不走 `submit()`,直接调内部纯函数**。

- `crates/douyin` 把 download / list_works / process 的**实际工作函数**(目前埋在 `*-worker` 子命令的 main 里)抽成 `pub` 异步函数,签名形如 `pub async fn download_one(input: DownloadInput) -> Result<DownloadOutput>` / `pub async fn list_works_page(...)` / `pub async fn process_one(...)`。
- 现有的 `submit()` + 隐藏子命令路径保持不动(本机 daemon / G10 单点跑继续依赖)。
- toolkit-worker 的 `run_job` 直接 `match kind { ... => douyin::download_one(input).await }`,不 spawn 子进程。生命周期 = lease 生命周期。
- **首阶段不动 douyin**,先把 THS 爬取按上述形态新建函数,验证模型;douyin 重构留给后续 kind 下放时再做。

### 0.3 跨节点的文件数据流(ASR 等)

**问题**:`asr-client::transcribe_path` 读本机路径;worker 与 controller 不共享 FS。子 job 输入写"本地文件路径"在分布式场景不成立。

**最小修正**:**按数据流形态显式分类 kind**。

- **绑定式 kind**(download + transcribe 同一 worker 连续执行):新增 `kind = "douyin_download_and_transcribe"`,单个 job 内 worker 顺序跑下载 → ASR → 上传整理稿 + 转写结果,本地文件全生命周期在 worker 临时目录,完成后 PUT artifact 回 controller 再清理。绑定模型避免跨节点搬大文件。
- **独立式 transcribe**(用户上传任意音频):worker 先 `GET /api/internal/artifacts/{job_id}/{name}` 从 controller(或日后对象存储)**拉文件**到本地临时目录,再调 `transcribe_path`。job input 是 artifact ref(`{ artifact_job_id, name }`)而非路径。
- 文档里所有写"本地文件路径"的 job input 都替换成上述两种形态之一,**禁止跨进程传裸路径**。
- 抖音管线的 download/transcribe 历史上就是同一进程跑的,绑定式 kind 是自然映射,不增加复杂度。

### 0.4 SQLite 迁移走真正的 v1→v2

**问题**:`migrations.rs::migrate` 只在 `meta.schema_version` 不存在时插入,**已有库的版本号永不更新**。新增 `jobs/workers` 表靠 `CREATE TABLE IF NOT EXISTS` 能建出来,但 `schema_version` 还是 `1`,后面再加迁移时无法判断起点。

**最小修正**:把 `migrate` 改成基于当前 `schema_version` 阶梯升级。**阶段 0 只引入 v2 分支**;阶段 1 加 THS 表时**追加 v3 分支**,不动 v2(否则已升到 2 的库读不到 ths 表,见 §11 阶段 1 / 修正 P1-a)。

阶段 0 完成后 `migrations.rs` 形态(只到 v2):

```rust
pub fn migrate(pool: &SqlitePool) -> Result<()> {
    let conn = pool.get()?;
    conn.execute_batch(DDL_V1)?;                                // 幂等基线
    let mut current: i64 = read_or_init_schema_version(&conn)?; // 不存在则写 1
    if current < 2 {
        apply_v2(&conn)?;                                       // 见下,处理 ALTER 的幂等
        conn.execute("UPDATE meta SET value='2' WHERE key='schema_version'", [])?;
        current = 2;                                            // 显式推进,后续分支基于此判断
    }
    // 阶段 1 在此处追加:
    // if current < 3 { apply_v3(&conn)?; bump; current = 3; }
    Ok(())
}
```

`let mut current` + 每段后 `current = N`(修正 P2-b):意图明确——「**当前已存在到哪个版本就写到哪个版本**」,实现者一眼看出每个 `if` 分支独立、不会误改成 `else if`。「一次启动 v1 → v3」会顺序执行 v2、v3 两个分支。

**ALTER TABLE 幂等(修正 P2-b)**:SQLite 的 `ALTER TABLE ... ADD COLUMN` **没有 `IF NOT EXISTS`**;如果上次迁移加列成功但版本号写失败,下次启动重跑会报 duplicate column 直接挂。`DDL_V2` 里所有 ALTER 必须走「先 PRAGMA 检查」:

```rust
fn apply_v2(conn: &Connection) -> Result<()> {
    conn.execute_batch(DDL_V2_CREATE_TABLES)?;  // jobs/workers/bootstrap_tokens + 全部 IF NOT EXISTS
    // ALTER tasks ADD COLUMN cancel_requested INTEGER NOT NULL DEFAULT 0
    if !column_exists(conn, "tasks", "cancel_requested")? {
        conn.execute("ALTER TABLE tasks ADD COLUMN cancel_requested INTEGER NOT NULL DEFAULT 0", [])?;
    }
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, col: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let cols: Vec<String> = stmt.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<_,_>>()?;
    Ok(cols.iter().any(|c| c == col))
}
```

`SCHEMA_VERSION` bump 到 `2`(阶段 0)→ `3`(阶段 1),`schema.rs` 加 `pub const DDL_V2_CREATE_TABLES: &str` / `pub const DDL_V3: &str`。后续每加一版照此 pattern,**新增表用 `CREATE TABLE IF NOT EXISTS`,加列走 PRAGMA 检查**。

### 0.5 jobs 表 schema 补齐 retry / cancel 字段

**问题**:前文协议说"`rate_limited` 指数退避"、"父任务 cancel 透传到子 job",但 §4.1 schema 没有承载字段。回队会立刻被下次 lease 抢走,取消只能临时塞进 progress 响应。

**字段补充**:

```sql
-- jobs 表新增列(DDL_V2)
not_before        INTEGER,           -- unix ms;lease 时过滤 not_before <= now,实现退避
error_kind        TEXT,              -- 'rate_limited' / 'network' / 'auth' / 'input' / 'panic' / null
cancel_requested  INTEGER NOT NULL DEFAULT 0,
cancelled_at      INTEGER,
```

- 失败回队时 controller 根据 `error_kind` 计算 `not_before = now + backoff(attempts, kind)`(rate_limited base 2s cap 5min;network base 1s cap 60s;auth/input 不回队直接 failed)。
- lease 查询加 `AND (not_before IS NULL OR not_before <= now)`。
- 父任务 cancel → aggregator 把同 parent 的 `queued` jobs `UPDATE state='cancelled', cancelled_at=now`,`leased` jobs `UPDATE cancel_requested=1`;`/jobs/progress` 响应 `cancel: true` 让 worker 优雅停;worker `complete(error_kind='cancelled')` 终结。
- jobs 终态加 `cancelled`。

### 0.6 Bootstrap token / 长期 token / config 下发的安全模型

**问题**:`register` 接受 `bootstrap_token` 但没说怎么签发、消费、绑定;`/workers/{id}/config` 把 cookie 明文下发给所有持长期 token 的 worker,缺最小权限。

**最小修正**:落到表 + 协议:

- 新增 `bootstrap_tokens` 表:`token_hash PK, allowed_caps JSON, allowed_channel TEXT, expires_at INTEGER, consumed_at INTEGER, created_by TEXT, notes TEXT`。
- 签发只能通过 controller 管理接口(本地 `/api/web/admin/bootstrap-tokens` 或 CLI 子命令),**指定预期 caps / channel / 过期时间**。
- 消费规则:单次消费(`consumed_at IS NULL`),`now < expires_at`,register 提交的 caps ⊆ allowed_caps,channel = allowed_channel。命中后:
  - 把 `consumed_at = now`、签发 `worker_token`(明文仅本次响应)、`workers` 行写 `token_hash = sha256(worker_token)` + `caps` + `channel`(以 token 限制为准,不信任 worker 自报)。
- **`/workers/{id}/config` 按 cap 过滤**:worker 只 caps 里没有 `cookie:acc_42` 就不下发该 cookie。THS 阶段 worker 拿到的 config 只有 `ths`/`asr` 端点这种**公开配置**,根本看不到 cookie——这是 §1 选 THS 优先的额外好处。
- 长期 token 吊销:管理接口 `POST /workers/{id}/revoke`,把 `token_hash` 置空 + `state='disabled'`,后续任何带该 token 的请求 401。
- 轮换:留 `POST /workers/{id}/rotate-token` 接口,worker 持旧 token 调用换新,旧的立即失效。首阶段先不做自动轮换,接口先占位。

### 0.7 Artifact 上传路径校验(实现细节修正)

**问题**:目标文件还不存在时,直接 canonicalize 完整目标路径会失败。

**正确写法**:文件名按白名单正则校验(`^[A-Za-z0-9._-]+$`,无 `/` `\` `..`)→ canonicalize **artifact 根目录 + job 子目录**(父目录,先 `create_dir_all`)→ 拼目标 = 父 + 文件名 → 断言父目录的 canonicalize 结果**前缀匹配** artifact 根目录。文档实现示例改为这个顺序。

### 0.8 Token / worker_id 绑定 + lease 续租策略

**问题(P2-a)**:`lease` 用 query 的 `worker_id`、`heartbeat` 用 path 的 `id`,如果 bearer token 解出的 `worker_id` 不和 path/query 强制核对,拿到一个 token 后可以冒充别的 worker 抢 lease。`caps` / `max_slots` / `channel` 若以客户端自报为准,worker 可以提权吃到不该接的 job。

**问题(P2-b)**:文档说 progress 顺便续租,但没规定 worker **必须**多久上报一次。任何超过 `lease_ttl` 无 progress 的 job 都会被 reaper 标 lost 重派,worker 上还在跑就会浪费算力 + 可能产生重复副作用。

**最小修正**:

- **所有 `/api/internal/*` 端点统一鉴权中间件**:bearer token → 查 `workers.token_hash` → 解出**绑定的 `worker_id`、caps、channel、max_slots、state`**。所有路径里出现的 `worker_id`(`/workers/{id}/...`)、query 里的 `worker_id`、body 里的 `worker_id` 必须 **== token 绑定的 id**,否则 403。
- **客户端传参只能收紧、不能放宽**:
  - `register` 传的 `caps` ⊆ bootstrap token 的 `allowed_caps`,DB 写入以**交集**为准(取 token allowed 与 register 自报的交集,不能超出 token 限制)。
  - heartbeat / lease 不接受 caps / max_slots / channel 字段(改了也不生效),要变就走 `rotate-token` 或 `revoke + 重签`。
  - lease query 里 `max` 不能超过 DB 里的 `max_slots`,超出按 `max_slots` 截断。
- **`max_slots` 强制执行**:lease 选择算法第 3 步「同 worker 当前 leased 数 < max_slots」,**slots 数以 DB 为准**,不是请求里带的 inflight 长度。worker 自报 inflight 仅供观测,不参与调度决策。
- **Lease 续租契约**:
  - `lease_ttl = 60s`,worker **必须**至少每 `lease_ttl / 3 = 20s` 上报一次 progress(也是续租)。
  - 推荐增加独立端点 `POST /jobs/{id}/renew { lease_id }` → `{ lease_until_ms, cancel }`,语义和 progress 的续租部分一致,但不强制要求 worker 报具体进度。worker 没东西可报时调 renew,纯保活。
  - `lease_ttl` / 续租间隔通过 controller config 可调,首阶段写死。
  - reaper(每 5s tick)按 `lease_until < now` 判 lost,不考虑 progress 时间戳——`lease_until` 才是续租契约的真相。

修订对 §5 协议的具体影响:lease/heartbeat/progress/complete/renew 端点描述都要标明「token 绑定 worker_id 校验」,并在 §5.2 加 `POST /jobs/{id}/renew` 端点定义。

---

**阶段 0 不动 agent / manifest / 灰度**,只把以上 8 点落进 `toolkit-core` / `toolkit-tasks` / `toolkit-server`。完成后阶段 1 才有合法的协议 / schema / 调用形态去填充。

## 2. 角色

| 角色 | 部署 | 公网 IP | 职责 |
|---|---|---|---|
| **Controller** | 现 `toolkit-server` | 必须有 | 任务拆分、`jobs` 表持久化、worker 注册/调度/收尾、artifact 落盘、Web UI / Agent API（不变）|
| **Agent**（bootstrap） | 每台 worker 一份，常驻 | 不要求 | 拉 manifest → 下载校验 worker 二进制 → 拉起并守护 → 回滚 |
| **Worker** | 由 Agent 拉起的子进程 | 不要求 | 长轮询 lease 子 job → 调业务 crate 的 pub async 函数(阶段 1 = `ths`;阶段 3+ 才扩到 `asr-client` / 部分 `douyin`)→ 上报进度 / 结果 / artifact |

**关键约束**：controller 必须有公网 IP；worker 只需出方向访问 controller。**所有连接由 worker 主动发起**，pull 模型。

## 3. 部署拓扑

```
[Controller — 公网 IP]                     [Worker 节点 1 — 独立出口 IP]
toolkit-server (axum)                       toolkit-agent (常驻)
 ├─ /api/web/*      (现状不变)               └─ exec → toolkit-worker
 ├─ /api/internal/* (worker 通道)                    ├─ 长轮询 lease/renew
 │   ├─ manifest                                     ├─ ths::list_constituents_page (阶段1)
 │   ├─ workers/register · heartbeat                 └─ artifact PUT 回 controller
 │   ├─ jobs/lease · renew · progress · complete
 │   └─ jobs/{id}/artifact/{name} (PUT/GET)  [Worker 节点 N — 另一个出口 IP]
 ├─ SQLite (toolkit.db)                      同上
 │   ├─ tasks (扩展:cancel_requested)
 │   ├─ jobs  (新增,子任务)
 │   ├─ workers (新增,节点元数据)
 │   └─ bootstrap_tokens (新增,§0.6)
 └─ <workspace>/artifacts/<job_id>/
```

**通道分离**：`/api/web/*` 保持给本机 Web UI / Agent；`/api/internal/*` 是 worker 专用通道，token 鉴权 + TLS，反向代理上加 IP allowlist 或限流，**逻辑上分两个 router**，将来想拆端口或拆进程都直接。

## 4. 任务模型

`toolkit-tasks` 按 §0.1 扩展(`RunOutcome::PendingExternal` + aggregator + cancel),并新增两张表(`jobs` / `workers`)和一张管理表(`bootstrap_tokens`)。

- **父任务**(`tasks` 表):业务编排。**派发段**写子 job 到 `jobs` 表(`INSERT OR IGNORE` by `idempotency_key`)后返回 `PendingExternal`,future 退出但 task 保持 `running`。**聚合段**由 aggregator tick 接管。父任务 kind 在 registry 注册时通过 `register_distributed::<T>()` 加入 `distributed_kinds` 白名单。
- **子 job**(`jobs` 表):worker 可独立完成的最小单元。kind 一一对应 worker 端 `match` 分支。**首阶段只有一个 kind**:
  - `ths_list_constituents_page`(THS 板块成分股翻页,匿名,见 §11 阶段 1)。
- **后续 kind 规划**(按 §0 数据流约束):
  - `douyin_list_works_page` / `douyin_detail` —— 需评估是否真的不依赖 cookie,确定后按 §0.2 抽 pub API,阶段 3 才动。
  - `douyin_download_and_transcribe` —— **绑定式**(§0.3),单 job 内同 worker 顺序跑下载 → ASR,临时文件全生命周期在 worker。
  - `asr_transcribe_artifact` —— **独立式**(§0.3),input 是 `{ artifact_job_id, name }` artifact ref,worker 先 GET artifact 再调 `transcribe_path`。
  - **明令禁止**:任何 job input 写裸文件路径。新增 kind 时按上述两种形态分类。

### 4.1 `jobs` 表 schema(DDL_V2)

```sql
CREATE TABLE IF NOT EXISTS jobs (
  id                TEXT PRIMARY KEY,        -- new_task_id()
  parent_task_id    TEXT NOT NULL,
  kind              TEXT NOT NULL,
  input             TEXT NOT NULL,           -- JSON;artifact ref / 业务参数,严禁裸路径
  state             TEXT NOT NULL,           -- queued/leased/succeeded/failed/lost/cancelled
  required_caps     TEXT NOT NULL DEFAULT '[]',
  affinity_key      TEXT,
  priority          INTEGER NOT NULL DEFAULT 0,
  idempotency_key   TEXT UNIQUE,             -- 例如 "ths_list_constituents_page:sector=XXX&page=3"
  assigned_worker   TEXT,
  lease_id          TEXT,
  lease_until       INTEGER,                 -- unix ms
  not_before        INTEGER,                 -- §0.5 退避;lease 时 not_before <= now
  attempts          INTEGER NOT NULL DEFAULT 0,
  max_attempts      INTEGER NOT NULL DEFAULT 3,
  error_kind        TEXT,                    -- §0.5 分类:rate_limited/network/auth/input/panic/cancelled/null
  last_error        TEXT,
  cancel_requested  INTEGER NOT NULL DEFAULT 0,
  cancelled_at      INTEGER,
  output            TEXT,
  progress          TEXT,
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS jobs_lease_idx     ON jobs(state, not_before, priority DESC, created_at);
CREATE INDEX IF NOT EXISTS jobs_parent_idx    ON jobs(parent_task_id);
CREATE INDEX IF NOT EXISTS jobs_reaper_idx    ON jobs(state, lease_until);
```

`idempotency_key` UNIQUE 让派发幂等(父任务重启重派直接 `INSERT OR IGNORE`)。Schema 通过 §0.4 的 `DDL_V2` + 显式版本号升级到 2 落入数据库。

### 4.2 `workers` 表 schema(DDL_V2)

```sql
CREATE TABLE IF NOT EXISTS workers (
  id              TEXT PRIMARY KEY,        -- worker 持久化的 UUID(由 bootstrap 流程生成)
  hostname        TEXT,
  egress_ip       TEXT,                    -- worker 自报,不从 TCP 源 IP 推断
  arch            TEXT NOT NULL,
  os              TEXT NOT NULL,
  agent_version   TEXT,
  worker_version  TEXT,
  caps            TEXT NOT NULL,           -- JSON;由 bootstrap_token.allowed_caps ∩ register 自报得出,§0.8
  max_slots       INTEGER NOT NULL DEFAULT 2,
  channel         TEXT NOT NULL DEFAULT 'stable',
  token_hash      TEXT,                    -- §0.6 长期 token 的 sha256;revoke 后 NULL
  state           TEXT NOT NULL,           -- online/offline/disabled
  last_heartbeat  INTEGER,
  registered_at   INTEGER NOT NULL,
  notes           TEXT
);
```

### 4.3 `bootstrap_tokens` 表 schema(DDL_V2)

```sql
CREATE TABLE IF NOT EXISTS bootstrap_tokens (
  token_hash      TEXT PRIMARY KEY,        -- sha256(明文 token)
  allowed_caps    TEXT NOT NULL,           -- JSON array;register 的 caps 必须 ⊆ 此集合
  allowed_channel TEXT NOT NULL,           -- 'stable'/'beta'
  expires_at      INTEGER NOT NULL,
  consumed_at     INTEGER,                 -- 单次消费;非 NULL 即失效
  created_by      TEXT,
  notes           TEXT,
  created_at      INTEGER NOT NULL
);
```

签发流程详见 §0.6。

**显式不存**:SSH key、密码、任何可直接拿到 worker shell 的凭据(§8)。

### 4.4 状态机

```
job:    queued ──lease──▶ leased ──complete(ok)──────────▶ succeeded
          ▲                  │
          │                  ├──complete(err 可重试,attempts<max)──▶ queued
          │                  │       (写 not_before=now+backoff;attempts 不递增)
          │                  │
          │                  ├──complete(err 不可重试 或 attempts>=max)──▶ failed
          │                  │
          │                  ├──complete(err_kind='cancelled')──▶ cancelled
          │                  │
          │                  └──lease_until < now──▶ reaper 回退 queued
          │                          (写 not_before=now+backoff(network);attempts 不递增)
          │
          └──parent cancel ── queued 状态 ──▶ cancelled (直接终态)

worker: registered ──heartbeat──▶ online ──60s 静默──▶ offline ──admin──▶ disabled
```

**计数规则(修正 P1-c)**:`attempts` **只在 `lease` 成功时递增**——退避回 queued / lost 回 queued 都不动 `attempts`。`attempts` 字面含义 = 「已被 lease 出去过的总次数」,而非「失败回队次数」。

- `lease`: `state=leased, assigned_worker=X, lease_id=NEW_UUID, lease_until=now+60s, attempts=attempts+1`;若 `attempts > max_attempts` 不应该出现在 queued 候选中(选择算法的过滤条件包含 `attempts < max_attempts`)。
- `progress` / `renew` / `complete` 必须带 `lease_id`;过期 lease 的 `complete` **409 拒收**,避免老结果覆盖新派发。
- **`error_kind` → 状态映射(统一表,修正 P1-b)**:

  | `error_kind`             | 可重试 | 终态(若不重试) | 备注 |
  |---|---|---|---|
  | `rate_limited`           | 是     | `failed`          | base 2s,cap 5min 指数退避 |
  | `network`                | 是     | `failed`          | base 1s,cap 60s |
  | `panic`                  | 是     | `failed`          | base 5s,cap 60s |
  | `auth` / `cookie_expired`| 否     | `failed`          | 直接 failed,通知父任务可能要换 cookie |
  | `input` / `parse`        | 否     | `failed`          | 不重试 |
  | `cancelled`              | 否     | **`cancelled`**   | 独立终态,不计入 `failed` |
  | (无 error_kind / ok)     | —      | `succeeded`       | |

  「可重试 + `attempts < max_attempts`」→ 回 `queued` 写 `not_before`;否则进上表的终态。

- reaper(每 5s)扫 `state='leased' AND lease_until < now`:
  - **先查 `cancel_requested`(修正 P1-a)**:若 = 1 → `state='cancelled', cancelled_at=now, error_kind='cancelled'`,**不回队**;
  - 否则等价于 worker 上报 `error_kind='network'` 的可重试失败 → 回队 + 写 `not_before` + **不动 attempts**(lease 时已经 +1 了)。
- **`cancelled` 独立终态**:worker 收到 `cancel=true` → 优雅停 → `complete(ok=false, error_kind='cancelled')` → `state='cancelled'`(**不是 failed**)。Aggregator 看到「全部子 job 进终态 + 至少一个 `cancelled`」时把父任务标 `mark_failed("cancelled")`(父用户视角是 fail,但子 job 状态保留真相,不被串改成 `failed`)。
- `failed` 终态的子 job 冒泡进父任务 `output.failures[]`(对齐 `refine` / `audio_forge`);`cancelled` 不进 failures[],单独进 `output.cancelled[]` 计数。

## 5. 通信协议

所有端点挂在 `/api/internal/`,JSON。**鉴权 / 绑定按 §0.8**:

- 统一 middleware:bearer token → `workers.token_hash` 查到唯一 `worker_id` + caps + max_slots + channel + state。404 / 401 早返回。
- path 里 `{worker_id}`、query / body 里 `worker_id` 必须 **== token 绑定 id**,否则 403。
- caps / max_slots / channel **以 DB 为准**,请求里带的同字段只能收紧不能放宽,改了不生效。
- **`/jobs/{id}/*` 端点的 job ownership 校验(修正 P2-a)**:`renew` / `progress` / `complete` / `artifact PUT` / `artifact GET` 的 URL 里不带 `worker_id`,服务端必须在校验 `lease_id` 前 / 同事务里**先校验 `jobs.assigned_worker == token.worker_id`**——否则不同 worker 拿到 `job_id + lease_id` 后能跨 worker 续租 / 提交 / 上传 artifact。任一不匹配 → 403。具体到 SQL,推荐 `UPDATE / SELECT ... WHERE id=? AND assigned_worker=? AND lease_id=?`,影响 0 行就 403/409。

### 5.1 注册与心跳

`POST /workers/register` —— **无需 bearer token**(还没签发),用 body 里的一次性 `bootstrap_token` 鉴权。

```jsonc
// req
{
  "worker_id": "w_abc123",       // worker 持久化的 UUID
  "hostname": "node-2",
  "egress_ip": "203.0.113.42",   // worker 自报(curl ifconfig.me),不从 TCP 源 IP 推断
  "arch": "aarch64-linux",
  "os": "ubuntu-22.04",
  "agent_version": "0.1.0",
  "worker_version": "0.4.2",
  "caps": ["ths"],               // 请求 caps;DB 写入 = 此 ∩ bootstrap_token.allowed_caps
  "max_slots": 2,                // 请求上限;DB 取 min(此, 服务端硬上限)
  "bootstrap_token": "..."       // 仅首次注册时携带,install.sh 注入
}
// resp
{
  "worker_token": "eyJ...",      // 长期 token,仅本次响应,worker 落本地;controller 只存 sha256
  "channel": "stable",           // 以 bootstrap_token.allowed_channel 为准,不接受 req 自报
  "effective_caps": ["ths"],     // 实际写入 DB 的 caps(交集结果)
  "config_revision": 7
}
```

bootstrap_token 单次消费,消费即写 `consumed_at`(§0.6 / §4.3)。重复 register 同 `worker_id`(已存在长期 token)→ 409,需要先 revoke。

`POST /workers/{worker_id}/heartbeat` —— 每 10s 一次,bearer token 鉴权。

```jsonc
// req
{
  "inflight": ["job_x", "job_y"],     // 仅供观测;不参与 slot 计数(以 DB leased 计数为准)
  "load": { "cpu": 0.3, "mem_mb": 412 },
  "worker_version": "0.4.2"
}
// resp
{
  "config_revision": 7,
  "please_exit_for_upgrade": false,
  "disabled": false,
  "cmds": []                          // 白名单运维指令(§8),首阶段空数组
}
```

### 5.2 任务 lease / 续租 / 上报 / 取消

`GET /jobs/lease?worker_id=w_abc123&kinds=ths_list_constituents_page&max=2` —— bearer token,`worker_id` 校验,`max` 截断到 DB `max_slots`。

- **长轮询**:有可派 job 立即返回;否则 hang 最多 30s。
- 选择算法(单事务,`BEGIN IMMEDIATE`)—— **`kind` 与 `caps` 是两个独立命名空间**(修正 P2-a):
  - `kind` = job 业务类型(如 `ths_list_constituents_page`),worker 通过 `req.kinds` 声明自己愿意接哪些 kind。
  - `caps` = worker 能力标签(如 `ths` / `cookie:acc_42`),job 通过 `required_caps` 声明自己需要哪些能力。
  - 两者**不互相检查**,完全正交。

  ```
  1. state='queued'
     AND attempts < max_attempts
     AND (not_before IS NULL OR not_before <= now)
     AND kind IN req.kinds                    -- 业务类型匹配
     AND json_subset(required_caps, worker.caps)  -- 能力子集
  2. (worker 当前 leased 数) + 已挑选数 < worker.max_slots
  3. 优先匹配 affinity_key 上次落在本 worker 的
  4. ORDER BY priority DESC, created_at ASC
  5. LIMIT min(req.max, worker.max_slots - 已 leased)
  6. UPDATE state='leased', assigned_worker=worker_id,
            lease_id=NEW_UUID, lease_until=now+60s, attempts=attempts+1
  ```

  阶段 1 具体例:worker `caps=["ths"]` + `req.kinds=["ths_list_constituents_page"]`,job `kind="ths_list_constituents_page"` + `required_caps=["ths"]` → 完美匹配,无需 kind/cap 跨命名空间映射。如果将来想限制「某 kind 必须由某类 worker 跑」,在 job 写 `required_caps`,而不是在 worker 写「我只接这个 kind」之外再加 kind→cap 映射。

```jsonc
// resp
{
  "jobs": [
    {
      "id": "job_x",
      "kind": "ths_list_constituents_page",
      "input": { "sector": "BK0420", "page": 3 },
      "lease_id": "lse_...",
      "lease_until_ms": 1733900000000,
      "traceparent": "00-...-...-01"
    }
  ]
}
```

`POST /jobs/{id}/renew` —— **纯保活**,worker 没东西可报时调。

```jsonc
// req
{ "lease_id": "lse_..." }
// resp
{ "lease_until_ms": ..., "cancel": false }
```

**Worker 续租契约(§0.8)**:`lease_ttl = 60s`,worker 必须至少每 `20s` 调一次 `progress` **或** `renew`。超期未续 → reaper 标 `lost`,worker 后续 `complete` 会 409。

`POST /jobs/{id}/progress` —— 上报进度顺带续租。

```jsonc
// req
{ "lease_id": "lse_...", "progress": { "step": "fetching", "page": 3 } }
// resp
{ "lease_until_ms": ..., "cancel": false }
```

`POST /jobs/{id}/complete`

```jsonc
// 成功
{ "lease_id": "lse_...", "ok": true, "output": { "items": [...] } }
// 失败
{ "lease_id": "lse_...", "ok": false, "error": "...", "error_kind": "rate_limited" }
```

`error_kind` 取值见 §4.4,controller 据此决定重试 / 退避 / 终结。`lease_id` 失效 → 409,worker 该 job 视为已被回收,不重试 complete。

### 5.3 Artifact 上传 / 下载

`PUT /jobs/{id}/artifact/{name}` —— 流式写 `<workspace>/artifacts/<job_id>/<name>`,header `X-Lease-Id` + `X-Sha256`。

**路径校验(§0.7)**:
1. `{name}` 正则白名单 `^[A-Za-z0-9._-]+$`,拒绝任何分隔符 / `..` / 盘符。
2. canonicalize artifact 根目录 + `job_id` 子目录(先 `create_dir_all`),得 `parent_canon`。
3. 断言 `parent_canon` 以 artifact 根目录 canonicalize 后的路径为**前缀**。
4. 目标 = `parent_canon + name`,流式写。

`GET /jobs/{id}/artifact/{name}` —— 独立式 transcribe(§0.3)从 controller 拉文件用。同路径校验。

父任务收尾时把 artifact 从 `<workspace>/artifacts/<job_id>/` 移到业务路径(`downloads/douyin/...` / `audioforge/...`),workspace 目录契约不变。

### 5.4 业务配置拉取(按 cap 过滤)

`GET /workers/{worker_id}/config` —— bearer token,`worker_id` 校验。

**关键:按 worker.caps 过滤 (§0.6)**。worker 只看得到自己 cap 允许的配置块:

```jsonc
// THS worker(caps=["ths"])看到的:
{
  "revision": 7,
  "ths": { "rate_limit_per_sec": 2 },
  "asr": { "endpoint": "http://gb10:9101/transcribe" }  // 若 caps 含 "asr"
}
// 没有 douyin 块,没有任何 cookie。worker 持长期 token 也拿不到。
```

cap → 配置块的映射在 controller 内显式枚举,**不在 worker 端做过滤**(worker 不可信)。worker 启动拉一次,heartbeat 看到 `config_revision` 变化再拉。

agent.toml 永远只有 `controller_url` / `worker_token` / `worker_id` / `channel` 这 4 个字段,业务配置全部走此端点。

## 6. 调度策略要点

- **能力路由(caps)**:把"哪个抖音账号 cookie 装在哪台 worker"建模成 `cap`(`cookie:acc_42`)。需该账号的子 job 在 `required_caps` 写明,只能落到拥有该 cap 的 worker。
- **Sticky affinity**:同一 creator 的翻页 / 同一 aweme 的多步骤(detail → download → transcribe),把 `affinity_key` 设为 `creator:sec_uid:...` 或 `aweme:...`,调度优先选上次跑过的 worker。理由:抖音 shadow-throttle 是出口 IP 维度,同一 creator 在多 IP 间反复横跳会增加被识别概率。
- **IP 维度限流**:`workers.egress_ip` 相同的多 worker(理论上极少)视为同一令牌桶,controller 端按 `egress_ip` 维度做 token bucket。
- **失败分类**:
  - `rate_limited` / `network` → 退避重试(指数,base 2s,cap 60s)。
  - `auth` / `cookie_expired` → 不重试,标 failed 并通知父任务(可能要换 cookie)。
  - `input` / `parse` → 不重试。
  - panic / 5xx → 重试(默认)。
- **Worker 离线**:heartbeat 缺失 60s → `state=offline`,inflight job 全部 `lost` 回队;`disabled` 状态的 worker `/jobs/lease` 直接返回空,不参与调度。

## 7. Worker 自更新(两段式 bootstrap)

**核心原则**:把"永远不变"和"频繁迭代"分两个二进制,降低升级风险面。

### 7.1 Agent(Stage 0)

- 体积 < 2MB,依赖少,接口稳定。
- 配置:`agent.toml` 只有 `controller_url`、`worker_token`、`worker_id`、`channel`(stable/beta) 4 字段。
- 主循环:

  ```
  loop {
    let m = GET /api/internal/workers/manifest?channel=...&arch=...&os=...;
    if local "./current/VERSION" != m.worker_version {
      download m.worker_url -> ./next/toolkit-worker.tar.gz
      verify sha256 == m.sha256 && minisign verify (公钥编译进 agent)
      atomic rename ./current -> ./prev, ./next -> ./current
    }
    let child = exec ./current/toolkit-worker
    wait child
    match child.exit_code {
      0          => continue,                       // 优雅退出,通常是升级窗口
      _ if just_upgraded && prev_exists => rollback,  // 新版 crash,自动回滚
      _          => exponential_backoff_retry,
    }
  }
  ```

- Agent 自身升级:走 systemd unit 里的 `ExecStartPre` 拉一次 agent channel 的 manifest,或者直接复用 `custom-utils` updater(`REPO_OWNER`/`REPO_NAME` 现已统一为 `toolkit`)。Agent 本身**半年级别**才动一次。

### 7.2 Worker(Stage 1)

- `register` 上报当前版本。
- heartbeat 收到 `please_exit_for_upgrade=true` → 停止 lease 新 job,等 inflight 全部 complete 或超过 graceful timeout(默认 5min) → `exit 0`。Agent 检测到 0 退出 → 拉新版 → 重启。
- worker 自身**不做**下载 / 校验 / 回滚,全交给 agent。

### 7.3 Manifest 端点

`GET /api/internal/workers/manifest?channel=stable&arch=aarch64-linux&os=linux`

```jsonc
{
  "agent_min_version": "0.1.0",
  "worker_version": "0.4.2",
  "worker_url": "https://github.com/jm-observer/toolkit/releases/download/v0.4.2/toolkit-worker-0.4.2-aarch64-linux.tar.gz",
  "sha256": "...",
  "signature": "...",                  // minisign 签名
  "rollout": { "percent": 50, "salt": "v0.4.2" }
}
```

- 灰度:`hash(worker_id + salt) % 100 < percent`,worker_id 稳定 → 同一台机器要么一直新版要么一直旧版,无震荡。
- 工件分发首阶段直接走 GitHub Release(沿用 `custom-utils` updater 的做法),controller manifest 只是转发 URL。后续 worker 不能直连公网时再让 controller 当 mirror(`<workspace>/artifacts/releases/`)。

### 7.4 一行 install

```bash
curl -fsSL https://controller.example.com/install.sh \
  | sh -s -- --controller https://controller.example.com --token <bootstrap-token>
```

脚本干:下载 agent 二进制 → 写 systemd unit → 写 `agent.toml`(含 bootstrap token) → `systemctl enable --now toolkit-agent`。首次注册后 worker 持久化长期 token,后续无人值守。

## 8. 凭据与安全边界

**Controller 存什么**(workers 表):IP、版本、能力、token 哈希、心跳、状态。

**Controller 不存什么**:SSH key、密码、任何能直接拿到 worker shell 的东西。

**SSH 通道**:独立于本系统,由人持有私钥,通过装机脚本写 `~/.ssh/authorized_keys`;worker 上 `PasswordAuthentication no`。需要集中管理就上 Tailscale SSH / Teleport / 堡垒机,**不在 toolkit 内造**。

**应用层运维通道**:替代 SSH 的日常运维。controller 通过 heartbeat 响应下发,agent / worker 执行白名单命令:

- `restart_worker`(worker 优雅 exit 0)
- `pull_manifest_now`(agent 立即检查更新)
- `reload_config`(worker 立即拉 `/workers/{id}/config`)
- `tail_log`(worker 把最近 N 行日志 POST 回 controller 的 `/workers/{id}/log-bundle`)

**没有任意命令执行**。白名单外的诉求 → 人肉 SSH,这是 break-glass,不是日常。

**网络面**:

- Controller `/api/internal/*` 必须 TLS(Let's Encrypt 就够),token 校验,反向代理(nginx/caddy)上加路径限流。
- Worker 出方向只到 controller 一个域名(加抖音 / FunASR 业务出口)。
- 签名公钥编译进 agent;manifest 的 `signature` 校验**不能 skip**。

**NAT 友好度 / 续租契约**:

- 心跳 10-15s(NAT 表项一般 30-120s 失活)。
- Lease 长轮询 hang ≤ 30s。
- **Lease TTL 60s,worker 必须每 ≤ 20s 调一次 progress/renew**(§0.8)。超期未续 → controller 回收重派,worker 后续 `complete` 收 409。
- 时间同步:`lease_until` / `not_before` 等时间字段一律以 controller 为准,worker 不参与时序判断,避免时钟漂移。

## 9. 观测

- **Trace**:复用 `TRACE_HUB_ENDPOINT`。父任务 span 在 lease 时把 `traceparent` 写入 job,worker 拉到后恢复 context → worker 端 span 是 controller 父任务的子节点,自然串起来。
- **指标**(controller 本地累计,可走 `/api/internal/metrics`):
  - `jobs_queued{kind}` / `jobs_inflight{kind}` / `jobs_failed_total{kind,error_kind}`
  - `worker_online_count` / `worker_inflight{worker_id}`
  - `lease_wait_seconds`(从 queued 到 leased 的耗时,反映吞吐)
  - `egress_ip_throttle_events`(根据 `error_kind=rate_limited` 聚合,识别哪个 IP 该歇)
- **日志**:worker 走 `custom-utils` logger,落本地文件;通过 `tail_log` 命令按需回收,不做实时聚合(首阶段)。

## 10. 不引入 MQTT 的理由(决策记录)

评估过 MQTT(EMQX shared subscription 天然实现 worker pool 抢任务),首阶段不采用:

- 量级:几台~几十台 worker、任务粒度秒级,HTTP 长轮询完全够。
- 双通道成本:大文件 artifact 不能走 MQTT,还是要 HTTP,等于双协议。
- 重做成本:鉴权 / trace 透传 / 限流 / 调试在 MQTT 主题模型里全要重想。
- broker 是新的单点 / 新的运维负担。

**重新评估触发条件**:

- worker 数 > 50,长轮询连接数成问题;
- 需要 controller **主动**给特定 worker 推紧急指令(比 heartbeat 间隔更快);
- worker 在极不稳定网络,需要 broker 的 LWT 快速发现离线。

届时形态:**控制面 MQTT、数据面 HTTP**,`jobs` 表与 idempotency 一个不动。

## 11. 分阶段落地

**阶段 0(1-2 天)** —— 协议 / 存储最小修正(见 §0):落 §0.1-§0.8 共 8 项前置,**不引入 worker 进程**,所有改动仍在 controller 内。

- `toolkit-tasks`:`TaskKind::run` 签名改 `-> Result<RunOutcome>`,所有现有 impl 扫一遍加 `Ok(RunOutcome::Done(v))` 包装(thin wrapper);`runner::run_task` 处理 `PendingExternal` 分支;`recover_interrupted` 加 `distributed_kinds` 白名单。
- `toolkit-core`:`schema.rs` 新增 `DDL_V2`(jobs / workers / bootstrap_tokens 三张表 + 各 index);`migrations.rs` 改为按 `schema_version` 阶梯升级,bump 到 2。
- `toolkit-server`:加 aggregator tick + reaper tick(两个独立 5s tokio interval);`/api/web/tasks/{id}/cancel` 端点;无 worker 进程的情况下 jobs 表保持空,所有现有任务行为等价。
- **验收**:`schema_version=2`,现有任务全绿;手动 INSERT 一条 distributed 父任务 + 几条 jobs 后 controller 重启,aggregator 能正确接管。

**阶段 1(THS 业务模块 + 调度骨架,1-2 天)** —— **首个分布式 kind = THS 板块成分股翻页**:

- **新建 `crates/ths` crate**(本仓库当前没有 THS 爬取代码,desktop / zero-desktop 那边只有登录态采集,不复用):
  - `pub async fn list_constituents_page(input: ListConstituentsPageInput) -> Result<ListConstituentsPageOutput>` —— 纯函数,直接 reqwest 调 THS web 接口,匿名(前几页无需登录,后几页本阶段不支持);返回结构化 stock 列表。
  - `pub async fn list_constituents_page_dispatch(plan: ListSectorPlan) -> Vec<JobSpec>` —— 业务规划函数,接「板块列表」吐「按板块 × page 拆分的子 job 输入」,供父任务派发段用。
  - HTTP client 复用 reqwest + 业务限流;不需要 cookie 配置块。
- **`toolkit-server` 新增 distributed kind 注册**:
  - 父任务 `ths_sync_sectors`(`distributed=true`):派发段调 `ths::list_constituents_page_dispatch` 写子 job 进 `jobs` 表 → 返回 `PendingExternal`。
  - 子 kind `ths_list_constituents_page` 在子 job worker 端 match。本阶段 worker 仍是 controller 进程内的 in-process worker(下一阶段才拆出去),directly 调 `ths::list_constituents_page`。
  - 数据落库:子 job `output.items` 在父任务收尾时由 aggregator 落到新表 `ths_constituents`(schema:`sector / stock_code / stock_name / rank / fetched_at / sector_page` + 复合主键 `(sector, stock_code)`,upsert)。**新增 `DDL_V3` 并把 `SCHEMA_VERSION` bump 到 3**(不能改 `DDL_V2`——阶段 0 跑过的库已经在 version 2,改 V2 不会再执行)。`migrations.rs` 加 `if current < 3 { execute(DDL_V3); UPDATE meta SET value='3' }` 阶梯。
- **`toolkit-server` `/api/internal/*` router**:register / heartbeat / lease / renew / progress / complete / artifact / config / manifest,按 §5 实现;bootstrap token 管理接口(`/api/web/admin/bootstrap-tokens` POST 签发 / GET 列表 / DELETE 吊销)。
- **验收**:本机起 controller,内嵌 in-process worker 挂 `caps=["ths"]`,提交 `ths_sync_sectors` 父任务,验证 派发 → lease → 多 page 并发抓取 → 聚合 → 父终态 → `ths_constituents` 表落库 链路通;cancel 接口生效;controller 重启 mid-job 后 aggregator 接管完成。

**阶段 2(真分布式,1-2 天)**:
- 新增 `crates/toolkit-worker` bin(依赖 `toolkit-core` + `ths` + `asr-client`):long-poll lease 主循环 + token 持久化 + egress IP 自探测 + `run_job` match dispatch + artifact 上传客户端。~200 行 Rust。
- 新增 `crates/toolkit-agent` bin:manifest 拉取 + 下载 + sha256 + minisign 校验 + 原子 rename + 守护 + 回滚。
- `install.sh` + systemd unit 模板;首阶段 release 直接走 GitHub。
- **验收**:部一台独立 IP 机器,挂 `caps=["ths"]`,跑同样的父任务,确认子 job 派到远端 worker;切回 in-process worker 行为等价(无回归)。

**阶段 3(能力扩展,1 天)**:
- ASR 绑定式 / 独立式 kind(§0.3)落地,验证 artifact 上传 / 下载通路。
- 评估抖音哪些接口确实不依赖 cookie(若有 → 按 §0.2 抽 pub API,新建 distributed kind);**依赖 cookie 的 douyin job 仍跑在 controller / 固定单 worker**。
- `error_kind` 分类退避实测(故意触发 THS 限流,看 `not_before` 是否生效)。
- affinity sticky 实测(同板块翻页固定到同 worker)。

**阶段 4** —— 规模化:扩到 N 台,IP 限流、健康面板、灰度 rollout 真用起来。视情况评估 MQTT 升级(§10 触发条件)。

## 12. 与现有模块的兼容

- **`toolkit-tasks`**:**有 breaking change**(§0.1)。`TaskKind::run` 签名 `-> Result<RunOutcome>`;所有现有 impl 加 `Ok(RunOutcome::Done(v))` 包装即可,不改业务逻辑。Registry 新增 `register_distributed::<T>()` 入口与 `distributed_kinds` 集合。`runner` 处理 `PendingExternal` + aggregator/reaper tick 是新增模块,不动现有 spawn / mark_succeeded / panic 捕获路径。**非分布式任务运行时行为完全等价**。
- **`douyin`** crate:**首阶段不动**(§0.2)。`submit()` + 隐藏子命令(`download-worker` / `list-works-worker` / `process-worker`)模型保留,本机 daemon / G10 单点继续用。**阶段 3 评估抖音哪些接口不依赖 cookie 后才动**——届时把对应工作函数从 `*-worker` 子命令 main 抽成 `pub async fn`,daemon 路径与 worker 路径并存。不强行把 douyin 全栈搬上分布式。
- **`asr-client`**:不动。绑定式 / 独立式 kind(§0.3)的两种数据流都通过 worker 端的协调代码使用,client 本身无感知。
- **`crates/ths`**:**新建**(§11 阶段 1)。本仓库当前 THS 代码只在 desktop 端做登录态采集,与新建的 `ths` crate 无重叠。`ths` crate = 匿名 HTTP 客户端 + 业务规划函数,无 desktop / Tauri 依赖,worker 可纯净链接。
- **现有 HTTP API**(`/api/web/*`):不动,Web UI / Agent 调用方零感知。新端点全部在 `/api/internal/*`(worker)+ `/api/web/admin/*`(管理 bootstrap_tokens 与 worker 状态)+ `/api/web/tasks/{id}/cancel`(取消)。
- **SQLite**:`schema_version` 真升到 2(§0.4),`DDL_V1` 不删,`DDL_V2` 增量加表 + 给 `tasks` 表加 `cancel_requested` 字段(`ALTER TABLE` in `DDL_V2`)。
- **`deploy-g10.ps1`**:`$Bins` 列表追加 `toolkit-agent` / `toolkit-worker`(如需在 G10 同机起 worker)。
- **自更新**:沿用 `custom-utils` updater,`REPO_OWNER`/`REPO_NAME` 已统一为 `jm-observer/toolkit`,manifest 端点转发 release URL 即可。

## 13. 未决问题(设计层)

- **同一账号 cookie 多 worker 共享**:目前模型是"一个 cookie 绑一台 worker"(cap 路由)。若后续要让多 worker 共享同一账号(更高吞吐),需要 controller 侧的 token bucket + cookie refresh 协调,留待数据驱动决策。
- **Artifact 大小**:单文件超过几百 MB 时直传 controller 不经济。届时引入对象存储(MinIO / S3),worker PUT presigned URL,controller 只记元数据。
- **跨地域**:首阶段所有 worker 在同一时区 / 同一 controller,跨地域延迟未评估。

## 14. 实操未决 / 风险登记(实现期再决)

下列条目**不影响协议形状 / schema**,但落地时必须给出明确策略。先记录,留待对应阶段开工时再裁决,避免重启设计辩论。

| # | 条目 | 触发阶段 | 当前倾向(可改) |
|---|---|---|---|
| 1 | **Aggregator/reaper tick 的并发安全**:若 controller 起多副本(HA),多个 tick 同时跑会双写 jobs/tasks 表。 | 阶段 0 | 首阶段 controller **单实例约束**,文档显式声明;HA 化时引入 advisory lock(SQLite `BEGIN IMMEDIATE` + 单写)或显式 leader 选举(如基于 `meta` 表的 lease)。 |
| 2 | **`bootstrap_tokens` 签发端点的鉴权**:`/api/web/admin/*` 目前没有鉴权框架。 | 阶段 1 | 首阶段走「本机访问 + 反代 IP allowlist」(controller 部署在内网/堡垒后,admin 端点只对管理员 IP 开放);后续若需公网 admin,加 admin token / OIDC。 |
| 3 | **`effective_caps` 交集为空**:`bootstrap_token.allowed_caps ∩ register.caps = ∅` 时写入无能力 worker 还是拒绝注册。 | 阶段 1 | **拒绝注册,返回 400** + 明确错误信息(无能力 worker 占着 `worker_id` 但接不到任何 job,是配置错误而非合法状态)。 |
| 4 | **manifest channel 与 worker 自报 channel 一致性**:worker 自报 `stable` 但 token 是 `beta`(或反之)。 | 阶段 2 | **以 DB(bootstrap_token 锁定的)channel 为准**——worker 自报 channel 字段直接忽略,manifest 端点查 `workers.channel` 而非请求参数。与 §0.8「客户端只能收紧不能放宽」原则一致。 |
| 5 | **lease 长轮询的 connection 限制**:阶段 4 worker 数 > 几十时,axum 的 keep-alive 连接 + 长轮询并发对内存 / fd 压力未评估。 | 阶段 4 | 首阶段不优化;>50 worker 时同步评估 MQTT 升级(§10 触发条件)。 |
| 6 | **artifact 流式上传的断点续传**:大文件传到一半 worker 重启,目前会从头再来。 | 阶段 3+ | 首阶段无续传(< 几百 MB 重传可接受);引入对象存储时(§13)同步获得续传能力,不在 controller 自造。 |
| 7 | **`config_revision` 的递增语义**:当前用 int 递增,谁来 bump、是否每次改 config 都 bump、worker 拉到旧 revision 时是否强制重启 worker。 | 阶段 2 | controller 内 config 写入用单调时钟(`registered_at` 之后的纳秒)而非业务计数;worker 拉到不同 revision **不强制重启**,只是下次拉 config 的触发器;config 内具体字段变化的影响留给 worker 自己判断(例如 `tts_endpoint` 变了重连即可,不需要重启)。 |
| 8 | **trace_id 在 jobs 表的持久化**:`traceparent` 当前只塞进 lease 响应,jobs 表里没有列。controller 重启后无法重建 parent span。 | 阶段 1 | 在 `jobs` 表加一列 `traceparent TEXT`,派发时和 input 一起写;lease 时透传。Schema 调整不大,可顺手加进 `DDL_V2`。**这条可能升级为 P2 修订**——等阶段 0 开工时确认。 |
| 9 | **worker 端日志回收的体积上限**:`tail_log` 命令把日志 POST 回 controller 时,日志可能很大。 | 阶段 3 | 命令参数指定行数上限(默认 200),controller 端单 worker 日志包大小硬上限(如 1 MB),超出截断并标注。 |
| 10 | **签名公钥的轮换**:agent 内嵌的 minisign 公钥若泄漏,需要 OTA 替换,但 agent 自更新自身又依赖该公钥。 | 阶段 4 | 首阶段不做密钥轮换(公钥不会主动泄漏);需要时走「agent 内嵌多公钥 + 任一签名通过即可」的过渡方案,或手动重装 agent。 |
