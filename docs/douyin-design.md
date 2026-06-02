# douyin crate 设计

## 0. 定位与文档约定

`crates/douyin` 是 zero Agent 工具库中的抖音工具。它不是通用网盘，也不是面向人工浏览的个人文件管理器，而是**面向 Agent 的任务型工具**。

本文档的主轴不是"抖音知识库这个产品"，而是一个更基础的问题：

> 当一个 Agent 工具要执行**长任务**（批量下载、爬取、转写）时，正确的工具定义是什么？

抖音下载只是这个问题的第一个实例。因此本文档先立"工具分层模型"和"长任务工具契约"两章作为主干（第 1、2 章），再展示抖音知识录入流水线如何落到这套契约上（第 3 章）。

合规边界：工具只应处理用户有权访问和保存的内容，不提供绕过平台权限、付费限制、DRM 或隐私边界的能力。

文档中每个能力点标注三种状态之一：

- **已实现** —— 当前代码已落地。
- **契约欠债** —— 属于"长任务工具契约"（第 2 章）要求、但当前机制尚未满足的项。这不是"未来可选"，而是已知的设计欠账。
- **🔭 机制演进** —— 补契约欠债的实现选项，可按痛点优先级推进（第 7 章）。

---

## 1. 工具分层模型

Agent 工具的第一要务是**可预测、可恢复、可查询**。不同任务对这三点的需求差异极大，因此 douyin 的命令分成两层。

### 1.1 立即返回工具

适合元信息抓取、轻量解析、单次查询。

特征：

- 单次调用内完成。
- stdout 输出一行紧凑 JSON。
- 业务失败也返回 `{error, error_kind}`，退出码保持 0。
- 不需要持久状态。
- Agent 不需要轮询。

### 1.2 长任务工具

适合批量下载、同步、爬取、模型/数据集缓存、批量处理。

特征：

- `submit` 立即返回 `job_id`（或兼容字段 `task_id`）。
- 实际工作由后台 worker / daemon 执行。
- Agent 通过 `status` / `list` / `retry` / `cancel` 查询和控制。
- 状态持久化，进程重启后能恢复。
- 输出使用原子写入，避免半成品污染结果。

### 1.3 为什么必须二分

长任务天然会遇到：**中断、重复、并发、网络失败、部分完成**。

如果把长下载做成一个普通命令，Agent 只能知道"这次进程结束了吗"——进程一旦被杀、网络一断，Agent 既不知道失败、也无法续传，更无从判重。

如果把它做成长任务工具，Agent 可以：提交任务、查询进度、重试失败、继续未完成内容。这正是"可预测、可恢复、可查询"的落点。

判定准则：一个能力只要**可能中断、可能重复、可能部分完成**，就应该做成长任务工具，而不是单次 CLI 调用硬跑到底。

### 1.4 现有命令归类

| 层 | 命令 |
|---|---|
| 立即返回 | `cookie-status` `set-cookie` `search-user` `resolve-user` `list-works` `list-tags` `filter-works` `publish-knowledge` `update` |
| 长任务（submit/status） | `download-submit`/`download-status`、`list-works-submit`/`list-works-status`、`process-submit`/`process-status` |

> 注意 `list-works` 同时存在立即返回版（同步拉取，适合小博主 / CLI 手测）与长任务版（`list-works-submit`，适合大博主 + callback）。两条路径产出的 work item 结构由 `enrich_with_tags` 保证一致。

---

## 2. 长任务工具契约

这是本设计的核心。一个长任务工具**必须**提供下列保证；当前机制未满足的项即为"契约欠债"，是第 7 章演进的来源。

### 2.1 接口面

```text
submit            创建任务，立即返回 job_id          —— 已实现
status <job_id>   查询单任务进度 + item 明细           —— 已实现
list-tasks [--state]  跨三类任务列摘要，按状态过滤     —— 已实现
*-retry <job_id>  重启任务，已完成项靠幂等跳过          —— 已实现
*-cancel <job_id> 请求取消，worker 处理下一条前转 cancelled —— 已实现
*-reap [--stale-secs] 扫描重启心跳超时的 running 任务   —— 已实现
```

submit 立即返回的目的是**避开 Agent 单次工具调用超时**（详见第 4 章）。

### 2.2 持久化与可恢复

状态落盘为文件，进程重启后能据此恢复，而非全靠内存。

- **已实现**：`<task_id>.job.json`（作业描述）+ `<task_id>.status.json`（进度），原子写（先 `.tmp` 再 rename），避免 Agent 读到半截 JSON。
- **契约欠债**：当前没有任何"重启后接管未完成任务"的逻辑。worker 是 fire-and-forget 子进程，崩了 status 永远停在 `running`，没人发现，更不会续跑。

### 2.3 幂等去重（回答："如何确保不重复下载？"）

当前去重是**靠文件存在判断**（`process.rs:244` transcript 存在则 skip；download 同理）。致命弱点：**崩溃残留的半截 mp4 会被误判为"已完成"**。正确的去重分三层：

| 层 | 机制 | 解决什么 | 状态 |
|---|---|---|---|
| L1 记录级账本 | submit 时按 ids 建全量 item 账本（state: queued/skipped），worker 消费——**去重看账本不看文件**，retry 据此只重跑未完成项 | 同任务内去重 + 崩溃/retry 后精确续传 | **已实现**（process：`build_ledger` + `recompute_counts`，worker 读现有账本 resume） |
| L2 文件级原子性 | 下到 `<id>.mp4.partial`，校验 `Content-Length` 后 atomic rename → `<id>.mp4`；空内容/长度不符判失败不落盘 | 崩溃残留的 `.partial` 不被误判完成（修掉当前最大隐患） | **已实现**（`download::finalize_download`） |
| L3 完整性校验 | 记录 `content_length` / `sha256`，重跑时校验而非盲信存在 | 防损坏文件、支持断点续传判断 | 契约欠债 |

L2 已落地：`download_one` 经 `finalize_download` 落盘，"最终 `.mp4` 存在"从此是可信的完成信号；process worker 复用同一路径。

### 2.4 失败可发现、可重启（回答："中断后如何发现失败并重启？"）

原来**完全没有**：worker 崩溃后 status 停在 `running`，无人知晓。**三类长任务（process / download / list-works）已全部补齐**：

1. **心跳**：worker running 期间每写一次 status 刷新 `heartbeat_at`（与 `updated_at` 区分——后者是"进度变更"，前者是"存活证明"）。`#[serde(default)]` 保证旧 status 文件仍可反序列化。
2. **判活**：`is_stale_running` = `state==running` 且心跳（缺则退化用 `updated_at`）距今 ≥ `stale_secs`；时间无法解析时保守判为非 stale，不误杀。
3. **谁来扫**：
   - 无 daemon（当前机制）：`{process,download,list-works}-reap [--stale-secs N]` 命令显式触发，把对应类型 stale 的 `running` 重启。各命令按 task_id 前缀（`dyproc` / `dy<digit>` / `dylw`）只认自己的任务，status 结构解析失败再做二次过滤。
   - 有 daemon（演进）：daemon 启动时自动扫，运行期定时扫。
4. **retry**：`{process,download,list-works}-retry --task-id X` 把 status 标回 `queued` 并重 spawn worker；worker 重建进度，已完成项靠幂等 skip（process 看 transcript 缓存、download 看 `.mp4`；list-works 无逐项缓存即整任务重跑）——不重下已完成内容。`reap` 内部即对每个 stale 任务调 `retry`。
5. **cancel**：`{process,download,list-works}-cancel --task-id X` 写 `<task_id>.cancel` 标志文件，worker 处理下一条（或翻下一页）前检查，命中即转 `cancelled` 并清标志干净退出。仅对 queued/running 有意义；终态任务返回 `not_cancellable`。`retry` 重跑前会清掉残留标志。

状态机（三类长任务统一）：

```text
queued -> running -> succeeded     全部成功
queued -> running -> partial       部分成功（done>0 且 failed>0）
queued -> running -> failed        全失败 / 前置失败（cookie 不可用等）
running -> stale -> queued         心跳超时被 reap 重新入队（演进）
running -> cancelled               收到 cancel 标志（已实现）
```

### 2.5 现状 vs 契约差距表

这张表是第 7 章 roadmap 的直接来源。

| 契约保证 | 状态 |
|---|---|
| submit → job_id | ✅ 已实现 |
| status 查询 + item 明细 | ✅ 已实现 |
| 状态持久化（job/status 文件 + 原子写） | ✅ 已实现 |
| 去重 L1 记录级账本 | ✅ process 已实现（submit 建账本、worker 消费、retry 精确续传）；download/list-works 可后续套用 |
| 去重 L2 `.partial` + atomic rename | ✅ 已实现（`download::finalize_download`） |
| 去重 L3 content_length/sha256 | ◐ 已校验 Content-Length；sha256 / 落盘记录待补 |
| 失败发现（heartbeat + stale 扫描） | ✅ 三类任务全实现（`heartbeat_at` + `{process,download,list-works}-reap`） |
| retry（重启任务，幂等续传） | ✅ 三类任务全实现（`{process,download,list-works}-retry`） |
| cancel | ✅ 三类任务全实现（`*-cancel` 写标志，worker 处理下一条前转 `cancelled`） |
| list（按状态过滤） | ✅ 统一 `list-tasks [--state]`（serde 忽略差异字段，跨三类一视图） |
| 持久 callback 队列 | ✅ 已实现（`callback.rs`：落盘 + 退避补发 + `callback-flush`） |
| 进程重启后恢复 | ✅ `douyin serve` daemon 启动即跑一轮 + 定时 reap/flush（`serve.rs`） |
| HTTP API（多入口复用） | ✅ MVP（axum：健康/列任务/查/retry/cancel/flush/maintenance） |

---

## 3. 产品实例：知识录入流水线

抖音知识录入是"长任务工具契约"的第一个完整实例，展示契约如何落到真实场景。

### 3.1 第一性设计决策：完整性由代码保证

目标：授权范围内，把单个博主的作品变成**可被 Agent 检索的、完整性可信的**本地知识库。

最关键的决策（`knowledge.rs:5`）：

> **完整性由 `for` 循环保证，不经任何 LLM 判断。** `publish_knowledge` 遍历 `works[]` 一条写一条 md，"列全 N 条"由循环保证。

这是为了在结构上消灭"LLM 漏列/省略 N 条作品"这一问题（known-issues #2）。它不是实现细节，而是整个流水线为什么这样切分的根本原因——把"完整性"从概率性的 LLM 输出里拿出来，交给确定性的代码。

### 3.2 端到端流水线

```text
list_works_submit                      列博主作品（长任务，带 callback）
   └─> works/<unique_id>.json          稳定缓存：sec_uid/nickname/aweme_count/throttled/works[]
        │
        ├─> list_tags                  聚合话题标签 + 计数（机械解析 desc 里的 #话题）
        ├─> filter_works               按标签筛选 → 匹配的 aweme_ids
        │
        └─> process_submit (ids)       逐条：下载 mp4 + ASR 转写（长任务，带 callback）
               └─> transcripts/<aweme_id>.json   {text, segments[], has_segments}
                    │
                    └─> publish_knowledge        遍历 works[] 一条写一条 md，有转写则回填
                           └─> knowledge/<unique_id>/
                                  ├── profile.md            博主资料
                                  ├── index.md              作品索引（时间倒序）
                                  └── transcripts/<id>.md   逐条：文案 + ASR 文本 + 字幕时间轴
```

### 3.3 各阶段输入/输出/落盘/幂等

| 阶段 | 输入 | 落盘 | 幂等策略 |
|---|---|---|---|
| `list_works_submit` | 主页 URL / 短链 / sec_uid | `works/<unique_id>.json` | 终态覆盖写（按 unique_id 稳定键） |
| `list_tags` / `filter_works` | unique_id (+ tags) | 无（纯读缓存） | 无副作用 |
| `process_submit` | aweme_id 列表 | `transcripts/<aweme_id>.json` | transcript 存在则 skip（见 §2.3 L2 欠债） |
| `publish_knowledge` | unique_id (+ only_ids) | `knowledge/<unique_id>/**` | 内容确定，重跑覆盖同名文件 |

### 3.4 知识包产物结构

```text
<knowledge_dir>/<unique_id>/
├── profile.md              # YAML frontmatter + 博主资料（含 throttled 告警）
├── index.md                # 作品清单，按 create_ym 倒序，链到各条目
└── transcripts/
    └── <aweme_id>.md       # frontmatter(tags/has_transcript/has_subtitle/asr_model)
                            #   ## 文案 / ## 视频内容(ASR) / ## 字幕(时间轴)
```

ASR 未就绪时各条目留"（待转写）"占位，待 `process` 跑完由 `publish_knowledge` 回填——这就是 callback 第二轮周期接管的典型动作（第 4 章）。

---

## 4. callback 驱动的两轮 Agent 循环

长任务"submit 立返"解决了不阻塞，但带来新问题：**任务跑完了，谁来接着做下一步？** 答案是 callback 驱动的第二轮 LLM 周期。

### 4.1 为什么需要

- submit 立即返回 → 避开 Agent 单次工具调用超时。
- 但 Agent 不应"一直轮询"等结果——那会占住一个昂贵的 LLM 上下文。
- 所以：worker 跑完终态后**主动 POST 回调** zero gateway，触发**第二轮独立的 LLM 周期**接管后续（如自动调 `publish_knowledge` 回填字幕）。

### 4.2 机制

worker 进入终态时 POST `http://127.0.0.1:9001/messages`：

```json
{
  "sender_id": "system:callback",
  "text": "<callback kind=\"douyin-process-done\" task_id=\"dyproc...\"/>",
  "metadata": {
    "callback": {
      "kind": "douyin-process-done",
      "payload": { "task_id": "...", "unique_id": "...", "session_id": "..." }
    },
    "delivery_handle": "dh_..."
  }
}
```

寻址参数用途：

- `delivery_handle`：回调寻址 handle，从主 Agent prompt 头部 `[Delivery]` 行取。**缺失则 worker 只落 status 不发回调**（CLI 手测场景）。
- `unique_id`：供第二轮精确回填对应博主的知识包。
- `session_id`：供 sps correlate 关联 sub-agent 调用链。

payload 动态组装：`null`/空字段不序列化（见 `process.rs:442`），避免下游收到无意义的 `null`。

### 4.3 防呆：delivery_handle 校验

`validate_delivery_handle`（`lib.rs:464`）在入队前拒绝：空串、非 `dh_` 前缀、含 `placeholder`/`demo` 的疑似占位符。意图是防止 Agent 把 prompt 模板里的占位符当真 handle 提交，导致任务跑完回调发往一个不存在的地址。

### 4.4 持久 callback 队列（已实现）

原欠债：worker 内 3 次重试若全失败、或 worker 在发回调前崩溃，**回调永久丢失**——任务完成了但发起方永远不知道。已用持久队列修复（`callback.rs`）：

1. **先落盘再投递**：worker 终态把回调入队为 `<task_id>.callback.json`（state=pending），再当场短重试 3 次（每次 5s）尝试送达。即使 worker 在投递前崩溃，记录已在盘上。
2. **状态机**：`pending →（attempt 累加 + 指数退避 next_retry_at）→ … → delivered`（送达）或 `failed`（超 `MAX_ATTEMPTS=8` 放弃）。
3. **补发**：`callback-flush` 命令（无 daemon 时由定时调用触发）扫描所有未送达记录，对到期（`next_retry_at ≤ now`）的各重投一次，更新状态。送达时同步把 `<task_id>.status.json` 的 `notified` 置 true（generic Value 更新，跨任务类型通用）。
4. **退避**：第 n 次失败后等 `30s · 2^min(n,6)`，封顶 ~32 分钟一次。

process 与 list-works 共用 `callback.rs`——body 结构、状态机、退避完全一致；payload 内容（process 带 unique_id+session_id、list-works 带 session_id）由各 worker 构造。

### 4.5 ⚠️ 待修：ADR 引用悬空

代码（`lib.rs:433` 注释）引用 `docs/adr/2026-05-31-callback-driven-async-tasks.md`，但该文件在本仓库不存在。需二选一：把 ADR 落进本仓，或把代码注释改为引用本章。

---

## 5. 任务与文件模型

### 5.1 三类任务统一视图

download / list-works / process 共享同一套模型：

```text
<task_dir>/
├── <task_id>.job.json      # 作业描述：submit 写、worker 读
└── <task_id>.status.json   # 进度：worker 增量原子写、status 命令读
```

`task_id` 形如 `dyproc<ms>` / `dy<ms>`，由 submit 时的毫秒时间戳生成。

### 5.2 status schema（process 为例）

```jsonc
{
  "task_id": "dyproc...",
  "state": "running",          // queued|running|succeeded|partial|failed
  "total": 10, "done": 6, "failed": 1, "skipped": 2,
  "results": [                 // item 账本（§2.3 L1）：submit 建全量、worker 消费
    { "aweme_id": "...", "state": "succeeded",
      "downloaded": true, "transcribed": true, "has_segments": true }
  ],
  "updated_at": "2026-...",
  "notified": false            // callback 是否已送达
}
```

**演进需新增字段**（对应 §2.4）：`heartbeat_at`、`attempt`、`next_retry_at`、item 级 `error_kind` / `content_length` / `sha256`。

### 5.3 worker 单进程内已收敛的健壮性

- 前置失败（cookie 缺失/不可用）→ 一次性 `write_all_failed`，所有 item 标失败，不空跑。
- 每条处理后增量 `write_status` → status 始终反映最新进度，Agent 中途查也准。
- 单条失败不影响其余条目 → 终态可能是 `partial`。

这些是"单进程存活期间"的健壮性；跨进程崩溃恢复仍是欠债（§2.2、§2.4）。

---

## 6. stdout 契约 / Cookie / 风控

### 6.1 stdout 契约

除隐藏 worker 和 `update` 外，正常命令只向 stdout 输出一行紧凑 JSON。业务失败仍输出 JSON，退出码保持 0：

```json
{"error":"cookie 缺少 msToken","error_kind":"cookie_missing"}
```

日志统一走 `custom-utils` logger。prod 构建启用 `--features prod`，日志落文件，stdout 保持干净。

### 6.2 Cookie 与访问身份

Cookie 文件由 `--cookie-file` 显式指定；未指定时按 `resolve_*` 规则回退：

```text
$ZERO_WORKSPACE/douyin/cookies.json
# ZERO_WORKSPACE 未设置时回退到 $HOME(/USERPROFILE)/.config/zero
```

文件支持 v1 结构 `{updated_at, value:{...}}` 或裸对象 `{...}`。

Cookie 属于敏感凭据：

- 不在 stdout / 日志中输出 Cookie 值。
- `cookie-status` 只返回字段数量、必要字段是否存在（msToken/s_v_web_id/ttwid + session）、登录态实测结果。

> 🔭 后续如引入通用 credential store，`douyin` 应改为引用 `credential_id`，由外部 Agent 包装层解析真实 Cookie。

### 6.3 风控与网络策略

抖音接口可能返回空 200、`verify_check`、抽稀列表、403、429。当前实现映射为 error_kind：`cookie_missing` / `anti_bot` / `not_found` / `network_error` / `api_failure` / `parse_error`。

初期策略：

- 不对疑似风控做高频自动重试。
- `search-user` 集群已被 verify_check 锁，降级为 anti_bot 引导（改用主页 URL）。
- `list-works` 返回 `throttled` 信号：判定 = `has_more 已结束 但 count < aweme_count * 0.9`（确定性抽稀，与每页平均条数无关）。告知调用方当前出口 IP 可能被抽稀。

> 🔭 服务化时增加 network profile（proxy / per-domain 并发 / 延迟 / 健康），按 domain + credential + profile 调度与熔断。

---

## 7. 机制演进：worker → daemon

第 2 章差距表里的"契约欠债"必须补，但**补的机制有两档**，可按痛点优先级推进，不必一步到位 daemon。

### 7.1 轻量档：CLI 触发的恢复，不引新依赖

在当前 spawn-worker 模型上即可补齐大部分契约：

1. **L2 原子下载**：`.partial` + content_length 校验 + atomic rename。（最高优先——决定判重可信）
2. **item 账本驱动**：submit 时建全量 item 记录，worker 消费账本而非靠文件存在判重。
3. **heartbeat + reap**：worker 定时写 `heartbeat_at`；新增 `reap` 命令（或任意 CLI 调用顺带）扫描 stale running 降回 queued。
4. **retry / cancel 命令**：retry 重跑非 done item；cancel 写标志文件。
5. **list 命令**：扫 task_dir 列任务，按 state 过滤。

这一档不需要常驻进程，适合现在就做。

### 7.2 完整档：daemon + HTTP API

当出现以下痛点时，再升级为常驻 daemon：

| 触发痛点 | 演进方案 |
|---|---|
| ~~callback 三次失败 / worker 发回调前崩溃 → 回调永久丢失~~ | ✅ 已在轻量档实现（§4.4 `callback.rs`，无需 daemon）。daemon 化后可由常驻进程定时 flush 替代手动/定时调用 |
| 跑了什么、何时、为何失败无法观测 | **event log**（append-only）：job.created/started、item.succeeded/failed、job.succeeded、callback.delivered。供 Web 时间线 / CLI events 查询 |
| CLI、Web、Agent 包装层要复用同一套任务接口 | ✅ **daemon + HTTP API（MVP 已实现，`serve.rs`）**：`douyin serve --bind 127.0.0.1:8787`，axum 路由 `GET /healthz`、`GET /v1/tasks[?state=]`、`GET /v1/tasks/{id}`、`POST /v1/tasks/{id}/retry\|cancel`、`POST /v1/callbacks/flush`、`POST /v1/maintenance/run`。启动即跑一轮维护 + 每 `--tick-secs` 定时 reap/flush，替代手动命令。**MVP 与 CLI 直接 spawn 并存**——尚未把 CLI submit 改为透传 daemon（创建类 `POST /v1/jobs` 待补），also event log（`GET /v1/events`）未做 |
| 本地用户要可视化运维 | **Web 模块**（`douyin serve --bind 127.0.0.1:8787`）：Jobs / Job Detail / Artifacts / Credentials / Network Profiles / Callbacks。默认只听 127.0.0.1、不展示 Cookie 原文；监听 0.0.0.0 须显式 token。候选 `axum` + `tower-http` |
| 下载产物要被多方按 id 引用 / 远程访问 | **artifact-store**：产物注册为 `{artifact_id, source, local_path, sha256, content_type}`，Agent 包装层按场景给 artifact_id / local_path / HTTP URL |

> daemon 未运行时，CLI 应返回明确错误而非静默降级为前台长任务（那会重新引入超时问题）：
> ```json
> {"error":"douyin service 未运行","error_kind":"service_unavailable","hint":"运行 douyin serve"}
> ```
> 开发场景可保留 `--local-worker` / 当前隐藏 worker 模式作为兼容路径。

### 7.3 推荐推进顺序（按痛点优先级，非平铺）

```text
✅ P0  L2 .partial + atomic rename        # 已完成：修掉"判重不可信"最大隐患
✅ P0  heartbeat + reap + retry (三类任务) # 已完成：process/download/list-works 全可发现、可重启
✅ P1  cancel + list-tasks                # 已完成：补齐长任务契约接口面
✅ P1  持久 callback 队列                  # 已完成：落盘 + 退避补发 + callback-flush
✅ P1  item 账本驱动去重 (process)         # 已完成：submit 建账本、worker 消费、retry 精确续传
✅ P2  daemon + HTTP API (MVP)            # 已完成：serve.rs，axum + 启动/定时自动 reap/flush
   P2  daemon submit (POST /v1/jobs)      # 把创建类任务也纳入 HTTP；CLI 透传 daemon
   P2  event log                         # 可观测性（GET /v1/events，Web 时间线消费）
   P3  Web 模块                           # 本地运维可视化（axum 静态资源 + tower-http）
   P3  credential store / artifact-store  # 解耦凭据与产物
```

P0/P1 多数落在"轻量档"，不引新依赖、可立即推进；P2 起才进入 daemon 形态。
已完成的两项 P0 即是范例：全程未引入新依赖，仅在现有 spawn-worker + 文件模型上补契约。
