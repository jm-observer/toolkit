# douyin CLI & HTTP API 参考

设计文档（`docs/douyin-design.md`）讲"为什么这样切"，本文档讲"怎么用"——逐命令的参数、返回 JSON、错误形态，以及 daemon HTTP API 与落盘文件契约。

约定：

- 所有 CLI 命令向 **stdout 输出一行紧凑 JSON**；业务失败也输出 `{error, error_kind}` 且退出码 0；日志走 logger（`--features prod` 落文件，不污染 stdout）。
- 涉及路径的参数缺省时按以下规则解析：`--cookie-file` → `$ZERO_WORKSPACE/douyin/cookies.json`；`--task-dir` → `$ZERO_WORKSPACE/douyin/tasks`；`--out-dir` → `$ZERO_WORKSPACE/downloads/douyin`；`--transcript-dir` → `$ZERO_WORKSPACE/douyin/transcripts`；`--works-dir` → `$ZERO_WORKSPACE/douyin/works`；`--knowledge-dir` → `$ZERO_WORKSPACE/knowledge/douyin`。`ZERO_WORKSPACE` 未设回退到 `$HOME(/USERPROFILE)/.config/zero`。
- 长任务命令（download / list-works / process 的 submit/status/retry/reap/cancel 以及 list-tasks / events / callback-flush）默认透传 daemon `http://127.0.0.1:8787`，可由 `DOUYIN_DAEMON=<url>` 覆盖；daemon 未运行返回 `service_unavailable`。
- `task_id` 前缀决定任务类型：`dy<digit>...` = download、`dylw...` = list-works、`dyproc...` = process。

---

## 1. 快速上手

完整流水线示例（授权博主 → 完整性可信的本地知识包）：

```bash
# 0. 一次性：把抖音浏览器 Cookie 写入工作区
douyin set-cookie --raw 'msToken=...; s_v_web_id=...; ttwid=...'
douyin cookie-status                            # 自检字段 + 登录态实测

# 1. 启动 daemon（本机开发；G10 由 systemd 拉起）
douyin daemon-start                             # 后台 spawn serve，幂等

# 2. 拉博主作品列表（长任务，立返 task_id）
douyin list-works-submit --input https://www.douyin.com/user/MS4wLjABAAAAxxx
#   → {"task_id":"dylw...","state":"queued","max_pages":60}
douyin list-works-status --task-id dylw...      # 终态 status 含完整 works[]

# 3.（可选）标签预筛
douyin list-tags --unique-id 82933463317
douyin filter-works --unique-id 82933463317 --tags ComfyUI,SD --match all
#   → {"matched":2,"aweme_ids":["7a","7b"], ...}

# 4. 下载 + ASR 合并任务（长任务）
douyin process-submit --ids 7a,7b
#   → {"task_id":"dyproc...","submitted":2,"skipped_already_done":0}
douyin process-status --task-id dyproc...

# 5. 把作品逐条机械写入知识包（完整性由 for 循环保证）
douyin publish-knowledge --unique-id 82933463317
#   → 落盘 <knowledge_dir>/82933463317/{profile,index,transcripts/<id>}.md
```

---

## 2. 命令分组

### 2.1 立即返回（不走 daemon）

| 命令 | 作用 |
|---|---|
| `cookie-status` | 自检 cookies.json 字段完整性 + 登录态实测 |
| `set-cookie --raw <s>` | 写入 cookies.json，支持 `k=v; k=v` 或 `{"k":"v"}` |
| `search-user --keyword <s> [--count 15]` | 按昵称/抖音号搜（已知被风控，多返回 `anti_bot`） |
| `resolve-user --input <url|短链|sec_uid>` | → 博主资料（含 `aweme_count`） |
| `list-works --input <...> [--max-pages 60]` | 同步列作品（小博主或调试用，大博主用 long-task 版） |
| `list-tags --unique-id <id>` | 聚合已拉作品的话题 + 计数 |
| `filter-works --unique-id <id> --tags <a,b> [--match all\|any]` | 按标签筛 aweme_ids |
| `publish-knowledge --unique-id <id> [--only-ids a,b]` | 逐条机械写入知识包（**完整性由 for 循环保证**） |

这些命令在 CLI 进程内同步完成、不依赖 daemon，业务失败仍返回 `{error, error_kind}` 退出码 0。

### 2.2 长任务（submit / status，透传 daemon）

每类任务都有同形态的 6 个命令：

| 模板 | 作用 |
|---|---|
| `<kind>-submit ... [--delivery-handle dh_xxx] [--session-id ...]` | 立返 task_id |
| `<kind>-status --task-id <id>` | 查任务进度（含 `heartbeat_at`） |
| `<kind>-retry --task-id <id>` | 重启任务（标回 queued + 重 spawn worker，已完成项幂等跳过） |
| `<kind>-cancel --task-id <id>` | 写 cancel 标志，worker 处理下一条前转 `cancelled` |
| `<kind>-reap [--stale-secs 600]` | 扫描重启心跳超时的 running 任务 |

其中 `<kind>` ∈ `{download, list-works, process}`。

**download-submit** —— 异步下载 mp4：

```bash
douyin download-submit --ids 7a,7b,7c
```

输入：`--ids <逗号分隔 aweme_id>`。返回：`{task_id, state: "queued", total}`。

幂等：`<out_dir>/<id>.mp4` 已存在则跳过（落盘走 `.partial` + atomic rename，崩溃残留 `.partial` 不会被误判）。

**list-works-submit** —— 异步列博主作品：

```bash
douyin list-works-submit --input <主页URL|短链|sec_uid> [--max-pages 60] \
                         [--delivery-handle dh_xxx] [--session-id ...]
```

返回：`{task_id, state: "queued", max_pages}`。终态 status 含完整 `works[]` + `aweme_count` + `throttled` + 持久缓存 `<works_dir>/<unique_id>.json`。

**process-submit** —— 下载 mp4 + ASR 转写合并任务：

```bash
douyin process-submit --ids 7a,7b \
                      [--asr-url http://127.0.0.1:9101/transcribe] \
                      [--asr-model funasr] [--vad] \
                      [--delivery-handle dh_xxx] [--unique-id ...] [--session-id ...]
```

返回：`{task_id, submitted, skipped_already_done}`。submit 时即按 ids 建全量 item 账本（已有 transcript 缓存的标 `skipped`，其余 `queued`），worker 消费——retry 据此精确续传，不重下已完成内容。

**delivery-handle 校验**：所有三类 submit 在入队前校验 `delivery_handle`（必须 `dh_` 前缀、非空、不含 `placeholder/demo`），不合格返回 `invalid_input`。

**status 返回示例**（process）：

```jsonc
{
  "task_id": "dyproc...",
  "state": "running",                // queued|running|succeeded|partial|failed|cancelled
  "total": 3, "done": 1, "failed": 0, "skipped": 1,
  "results": [                        // item 账本（§2.3 L1）
    { "aweme_id": "7a", "state": "skipped",   "downloaded": true,  "transcribed": true,  "has_segments": false },
    { "aweme_id": "7b", "state": "succeeded", "downloaded": true,  "transcribed": true,  "has_segments": true  },
    { "aweme_id": "7c", "state": "queued",    "downloaded": false, "transcribed": false, "has_segments": false }
  ],
  "updated_at":   "2026-06-02T10:00:01Z",
  "heartbeat_at": "2026-06-02T10:00:01Z",   // worker 存活证明
  "notified": false                          // gateway callback 是否送达
}
```

不同任务的非公共字段：
- download：`files[]` / `errors[]`（不走账本模型）
- list-works：`sec_uid` / `nickname` / `unique_id` / `pages_fetched` / `aweme_count` / `count` / `throttled` / `works[]`

### 2.3 跨任务运维

| 命令 | 作用 |
|---|---|
| `list-tasks [--state <s>]` | 跨三类列任务摘要（task_id/kind/state/updated_at/heartbeat_at），按状态过滤；按 `updated_at` 倒序 |
| `events --task-id <id>` | 读 append-only 事件时间线（`<task_id>.events.jsonl`） |
| `callback-flush` | 扫描未送达 callback，对到期的重投一次 |

`list-tasks` 返回示例：

```json
{ "count": 2, "tasks": [
  { "task_id": "dyproc...", "kind": "process",    "state": "running",   "updated_at": "...", "heartbeat_at": "..." },
  { "task_id": "dylw...",   "kind": "list-works", "state": "succeeded", "updated_at": "...", "heartbeat_at": "..." }
] }
```

`events --task-id` 返回示例：

```json
{ "task_id": "dyproc...", "count": 4, "events": [
  { "ts": "...", "task_id": "dyproc...", "event": "job.created" },
  { "ts": "...", "task_id": "dyproc...", "event": "job.started" },
  { "ts": "...", "task_id": "dyproc...", "event": "item.succeeded", "detail": { "aweme_id": "7a" } },
  { "ts": "...", "task_id": "dyproc...", "event": "job.succeeded" }
]}
```

事件种类：`job.created` / `job.started` / `item.succeeded` / `item.skipped` / `item.failed` / `job.succeeded` / `job.partial` / `job.failed` / `job.cancelled` / `callback.delivered` / `callback.failed`。

### 2.4 Daemon & 部署

| 命令 | 作用 |
|---|---|
| `serve [--bind 127.0.0.1:8787] [--tick-secs 60] [--stale-secs 600]` | 前台启动 daemon（HTTP API + 启动/定时维护） |
| `daemon-start [--bind ...] [--wait-secs 10]` | 后台 detached spawn `serve`，幂等（已活返回 `already_running`） |
| `daemon-status` | probe `/healthz`，返回 `{alive, daemon_url}` |
| `install [--dry-run\|-n] [--workspace\|-w <path>]` | G10 部署：systemd 用户级服务（rootless） |
| `update [--force\|-f]` | 自更新二进制（`~/.local/bin/douyin`） |

`daemon-status` 返回示例：

```json
{ "alive": true, "daemon_url": "http://127.0.0.1:8787" }
```

`install --dry-run` 渲染 systemd unit（详见 design §8）；`install` 直接落盘 + `daemon-reload` + `enable` + `enable-linger`。`update` 走 `LinuxService` 拉最新 GitHub Release 资产（按 `name.contains(bin) && name.contains(target)` 匹配 `douyin_<target>`，target 由当前架构决定）。

---

## 3. HTTP API（`douyin serve`）

启动后所有路由都在 `http://<bind>` 下（默认 `127.0.0.1:8787`）。安全默认：仅监听本机；不展示 cookie/凭据原文。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET  | `/` | 内嵌运维面板（HTML + JS 轮询） |
| GET  | `/healthz` | `{ok:true, service:"douyin"}` |
| POST | `/v1/jobs` | 创建任务（三类统一入口） |
| GET  | `/v1/tasks[?state=<s>]` | 跨三类列任务摘要 |
| GET  | `/v1/tasks/{task_id}` | 单任务 status |
| GET  | `/v1/tasks/{task_id}/events` | 单任务 event log |
| POST | `/v1/tasks/{task_id}/retry` | 重启 |
| POST | `/v1/tasks/{task_id}/cancel` | 取消 |
| POST | `/v1/callbacks/flush` | 扫描补发未送达 callback |
| POST | `/v1/maintenance/run` | 立即跑一轮 reap（三类） + callback flush |

### 3.1 POST /v1/jobs

```jsonc
// 请求 body
{
  "kind": "douyin.process",          // 或 process | download | douyin.download
                                     //   | list-works | douyin.list-works
  "params": {
    "ids": ["7a", "7b"],             // process / download
    // process 特有：asr_url / asr_model / vad / unique_id
    "delivery_handle": "dh_xxx",     // 三类共用（含校验）
    "session_id": "...",             // 三类共用
    // list-works 特有：input / max_pages
    // 路径覆盖（可选）：cookie_file / out_dir / transcript_dir
  }
}
```

响应即对应 `*-submit` 的 JSON 输出（含 `task_id`）。未知 kind → `{error, error_kind: "invalid_input"}`。

### 3.2 维护端点

- `POST /v1/maintenance/run` 立即调用 `run_maintenance(stale_secs=serve --stale-secs)`，等同 `process-reap + download-reap + list-works-reap + callback-flush` 一次性执行：

```json
{ "reaped": { "process": [], "download": [], "list_works": [] },
  "callbacks": { "delivered": 0, "pending": 0, "failed": 0 } }
```

daemon 启动即跑一轮 + 每 `--tick-secs` 定时跑一轮。

### 3.3 内嵌面板（GET `/`）

`dashboard.html` 嵌入二进制（`include_str!`），无新依赖。轮询 `/v1/tasks`（3s/次），可按状态过滤、查 detail、retry/cancel/flush 操作直连后端。仅本机 7 列 + JSON 详情区，足以做"任务时间线 + 状态"运维。

---

## 4. 落盘文件契约（`<task_dir>/`）

```
<task_id>.job.json         submit 时写、worker 读，作业描述
<task_id>.status.json      worker 增量原子写，status 命令/HTTP 读
<task_id>.events.jsonl     append-only 事件流，events 命令/HTTP 读
<task_id>.callback.json    持久 callback 记录（pending|delivered|failed）
<task_id>.cancel           cancel 标志（空文件存在即触发），worker 处理下一条前检查
```

附加（process 任务）：

```
<transcript_dir>/<aweme_id>.json     ASR 转写缓存 {text, segments[], has_segments, asr_model, transcribed_at}
<out_dir>/<aweme_id>.mp4             无水印视频；落盘走 .mp4.partial → atomic rename
<works_dir>/<unique_id>.json         list-works 终态稳定缓存（list-tags/filter/publish 共用）
<knowledge_dir>/<unique_id>/         publish-knowledge 产物（profile + index + transcripts/）
```

### 4.1 状态机（三类任务统一）

```
queued -> running -> succeeded     全部成功（process: done == total && failed == 0）
queued -> running -> partial       部分成功（done>0 且 failed>0；或 list-works 抽稀）
queued -> running -> failed        前置失败 / 全失败
running -> stale -> queued         心跳超时被 reap 重新入队
queued/running -> cancelled        收到 cancel 标志
```

### 4.2 CallbackRecord schema

```jsonc
{
  "callback_id": "<task_id>",        // 一任务一回调
  "task_id":     "<task_id>",
  "kind":        "douyin-process-done",    // 或 -failed / list-works-done / -failed
  "delivery_handle": "dh_xxx",
  "payload": {                       // 至少含 task_id；按类型可含 unique_id/session_id
    "task_id":   "dyproc...",
    "unique_id": "82933463317",      // process 才有
    "session_id":"..."
  },
  "state":   "pending",              // pending | delivered | failed
  "attempt": 0,                      // 投递尝试次数
  "last_error": null,
  "next_retry_at": null,             // 退避截止时间；pending 才有
  "created_at":   "...",
  "delivered_at": null
}
```

退避：第 n 次失败后等 `30 * 2^min(n, 6)` 秒（封顶 ~32 分钟），达 `MAX_ATTEMPTS=8` 标 `failed` 停止补发。

### 4.3 Event schema

```jsonc
{
  "ts":      "2026-06-02T10:00:00Z",
  "task_id": "dyproc...",
  "event":   "item.succeeded",
  "detail":  { "aweme_id": "7a" }    // 可选
}
```

写入 best-effort——记日志失败只 warn，绝不阻断任务本身。

---

## 5. 错误码（`error_kind`）

| `error_kind` | 触发场景 |
|---|---|
| `invalid_input` | 参数缺失/格式错（ids 为空、delivery_handle 不合规、未知 kind 等） |
| `not_found` | 任务不存在 / 作品已删 / sec_uid 解析无果 |
| `not_cancellable` | 任务已终态，cancel 无意义 |
| `not_listed` | 未先跑过 list-works（list-tags / filter-works / publish 无缓存可用） |
| `service_unavailable` | daemon 未运行（hint 含 `daemon-start` / `serve` / `systemctl` 三条启动方式） |
| `cookie_missing` | cookie 缺必要字段（msToken/s_v_web_id/ttwid + session） |
| `anti_bot` | 被风控（verify_check / 空 200 / search 集群已锁） |
| `network_error` / `api_failure` / `parse_error` | 网络层 / 接口层 / 解析层 |
| `timeout` | daemon-start 后 wait_secs 内未就绪 |
| `internal` | HTTP 层意外错误（路由 handler 内 anyhow） |

---

## 6. 环境变量

| 变量 | 用途 |
|---|---|
| `ZERO_WORKSPACE` | 工作区根（默认 `$HOME(/USERPROFILE)/.config/zero`） |
| `DOUYIN_DAEMON` | 长任务 CLI 透传的 daemon base URL（默认 `http://127.0.0.1:8787`） |

---

## 7. 与设计文档的映射

| 本文档 § | 设计文档对应 |
|---|---|
| §1 / §2.1 立即返回 | 设计 §1 工具分层模型 |
| §2.2 长任务 | 设计 §2 长任务工具契约 + §5 任务文件模型 |
| §2.3 events / callback-flush | 设计 §4.4 持久 callback 队列 + §Event Log |
| §2.4 install / serve | 设计 §7 演进路线 + §8 G10 部署 |
| §3 HTTP API | 设计 §7.2 完整档 |
| §4 落盘契约 | 设计 §2.3 去重三层 + §5 文件模型 |
| §5 错误码 | 设计 §6.3 风控与网络策略 |
