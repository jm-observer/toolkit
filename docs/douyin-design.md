# douyin crate 初步设计

## 定位

`crates/douyin` 是 zero Agent 工具库中的抖音工具。它不是通用网盘，也不是面向人工浏览的个人文件管理器，而是面向 Agent 的任务型下载与内容处理入口。

当前目标聚焦在经过授权的内容处理场景：

- 维护抖音 Web 访问所需的 Cookie。
- 解析博主主页 URL / 短链 / `sec_uid`。
- 拉取博主作品元数据。
- 按 `aweme_id` 异步下载视频文件。
- 可选地串联本地 ASR 服务，生成转写缓存，再写入知识包。

工具只应处理用户有权访问和保存的内容，不提供绕过平台权限、付费限制、DRM 或隐私边界的能力。

## 工具形态

`douyin` 使用两类命令。

立即返回命令：

- `cookie-status`
- `set-cookie`
- `search-user`
- `resolve-user`
- `list-works`
- `list-tags`
- `filter-works`
- `publish-knowledge`
- `update`

任务型命令：

- `download-submit` / `download-status`
- `list-works-submit` / `list-works-status`
- `process-submit` / `process-status`

任务型命令采用 submit/status 模式：submit 只创建任务并立即返回 `task_id`，实际工作由同一二进制的隐藏 worker 子命令执行。这样可以避开 Agent 单次工具调用超时，也给后续服务化留下接口。

## CLI 与服务交互

服务化后，`douyin` 应保持 CLI 作为 Agent 的稳定入口，但实际任务调度交给本地 daemon。

推荐交互模式：

```text
Agent -> douyin CLI -> local daemon -> task store / worker / artifact store
```

CLI 的职责：

- 解析命令行参数。
- 读取用户可见配置，例如 service endpoint。
- 调用本地 daemon API。
- 将 daemon 响应压缩为一行 JSON 输出到 stdout。
- 当 daemon 未运行时，返回明确错误或按配置尝试拉起服务。

daemon 的职责：

- 持久化 job / item / artifact / callback 状态。
- 调度 worker。
- 维护 Cookie / network profile 健康状态。
- 执行任务恢复、重试、取消和清理。
- 向 Web 模块提供任务查询 API。

默认通信方式建议使用本机 HTTP：

```text
http://127.0.0.1:8787
```

原因是 CLI、Web UI、Agent 包装层都能复用同一套接口；后续如果要支持跨进程或远程访问，只需要在 daemon 层增加认证和监听配置。

CLI 到 daemon 的 API 草案：

```text
POST /v1/jobs
GET  /v1/jobs/{job_id}
GET  /v1/jobs?state=running
POST /v1/jobs/{job_id}/retry
POST /v1/jobs/{job_id}/cancel
GET  /v1/artifacts/{artifact_id}
GET  /v1/artifacts/{artifact_id}/download
```

`download-submit` 对应：

```http
POST /v1/jobs
```

请求：

```json
{
  "kind": "douyin.download",
  "params": {
    "aweme_ids": ["123", "456"],
    "credential_id": "douyin-main",
    "out_dir": "..."
  },
  "notify": {
    "mode": "webhook",
    "url": "http://127.0.0.1:9001/messages",
    "delivery_handle": "dh_..."
  }
}
```

响应：

```json
{"job_id":"job_...","state":"queued","total":2}
```

CLI 输出应基本透传这个响应：

```json
{"task_id":"job_...","state":"queued","total":2}
```

为了兼容当前实现，可以保留 `task_id` 字段；服务内部可以统一使用 `job_id`，CLI 输出时同时给出二者：

```json
{"task_id":"job_...","job_id":"job_...","state":"queued","total":2}
```

daemon 未运行时，CLI 应返回：

```json
{"error":"douyin service 未运行","error_kind":"service_unavailable","hint":"运行 douyin serve"}
```

不建议 CLI 在生产环境静默降级为前台长任务，因为这会重新引入 Agent 调用超时问题。开发场景可以提供 `--local-worker` 或继续保留当前隐藏 worker 模式作为兼容路径。

## stdout 契约

除隐藏 worker 和 `update` 外，正常命令只向 stdout 输出一行紧凑 JSON。业务失败仍输出 JSON，退出码保持 0：

```json
{"error":"cookie 缺少 msToken","error_kind":"cookie_missing"}
```

日志统一走 `custom-utils` logger。prod 构建启用 `--features prod`，日志落文件，stdout 保持干净。

## Cookie 与访问身份

Cookie 文件由 `--cookie-file` 显式指定；未指定时解析到：

```text
$ZERO_WORKSPACE/douyin/cookies.json
```

文件支持两种形态：

```json
{"updated_at":"...","value":{"msToken":"...","s_v_web_id":"..."}}
```

或裸对象：

```json
{"msToken":"...","s_v_web_id":"..."}
```

Cookie 属于敏感凭据：

- 不在 stdout 中输出 Cookie 值。
- 不在日志中输出 Cookie 值。
- `cookie-status` 只返回字段数量、必要字段是否存在、登录态探测结果。

后续如果引入通用 credential store，`douyin` 应改为引用 `credential_id`，由外部 Agent 包装层解析真实 Cookie。

## 下载任务模型

当前 `download-submit` 的输入是 `aweme_id` 列表：

```text
douyin download-submit --ids 123,456 --cookie-file <path> --task-dir <path> --out-dir <path>
```

返回：

```json
{"task_id":"dy...","state":"queued","total":2}
```

worker 逐个处理：

1. 读取 job 文件。
2. 读取 Cookie。
3. 通过 `aweme_detail` 获取 `play_addr` URL。
4. 下载到 `<out_dir>/<aweme_id>.mp4`。
5. 增量原子写入 status。

状态机：

```text
queued -> running -> succeeded
queued -> running -> partial
queued -> running -> failed
```

当前下载幂等策略是：目标 mp4 已存在则跳过下载并返回已有路径。后续应增强为 `.partial` 写入、完成后 atomic rename，并记录 `content_length` / `sha256`。

## 任务状态与恢复

任务目录中保存两类文件：

```text
<task_id>.job.json
<task_id>.status.json
```

status 使用临时文件加 rename 的方式原子替换，避免 Agent 读到半截 JSON。

当前恢复能力是“可查询已完成/部分完成状态”。下一阶段如果改为常驻服务，应增加：

- `heartbeat_at`
- `attempt`
- `next_retry_at`
- `error_kind`
- stale running 任务扫描
- `retry <task_id>`
- `cancel <task_id>`

服务启动时应把 heartbeat 超时的 `running` 任务重新判定为 `queued` 或 `failed`。

## 风控与网络策略

抖音相关接口可能返回空 200、`verify_check`、抽稀列表、403、429 或其他风控信号。当前实现将部分信号映射为：

- `cookie_missing`
- `anti_bot`
- `not_found`
- `network_error`
- `api_failure`
- `parse_error`

初期策略：

- 不对疑似风控做高频自动重试。
- `search-user` 被风控时，引导使用主页 URL。
- `list-works` 返回 `throttled` 信号，告知调用方当前出口 IP 可能被抽稀。

后续服务化时，应增加 network profile：

```text
network_profile -> proxy / per-domain concurrency / delay / health
```

并按 domain + credential + network_profile 进行调度与熔断。

## 文件交付

当前交付方式是本地路径：

```json
{"files":["D:\\...\\123.mp4"]}
```

这适合同机 Agent 和本地工具链。未来如果引入通用 `artifact-store`，`douyin` 不应直接承担个人存储职责，而应作为生产者注册 artifact：

```json
{
  "artifact_id": "art_...",
  "source": {"kind": "douyin", "aweme_id": "123"},
  "local_path": "...",
  "sha256": "...",
  "content_type": "video/mp4"
}
```

Agent 包装层再根据场景提供 `artifact_id`、`local_path` 或 HTTP URL。

## 任务完成通知

任务完成通知不应只依赖“调用方一直轮询”。推荐采用 webhook + event log 双轨。

### Webhook

submit 时调用方可以附带通知配置：

```json
{
  "notify": {
    "mode": "webhook",
    "url": "http://127.0.0.1:9001/messages",
    "delivery_handle": "dh_...",
    "session_id": "sess-...",
    "events": ["succeeded", "partial", "failed"]
  }
}
```

任务进入终态时，daemon POST：

```json
{
  "sender_id": "system:callback",
  "text": "<callback kind=\"douyin-download-done\" task_id=\"job_...\"/>",
  "metadata": {
    "callback": {
      "kind": "douyin-download-done",
      "payload": {
        "job_id": "job_...",
        "task_id": "job_...",
        "state": "succeeded",
        "artifact_ids": ["art_..."],
        "session_id": "sess-..."
      }
    },
    "delivery_handle": "dh_..."
  }
}
```

通知发送需要持久化状态：

```text
pending -> sending -> delivered
pending -> sending -> failed
failed -> pending
```

至少记录：

- `callback_id`
- `job_id`
- `event`
- `target_url`
- `attempt`
- `last_error`
- `next_retry_at`
- `delivered_at`

这样 daemon 重启后可以继续补发未送达通知，避免任务完成但发布方永远不知道。

### Event Log

daemon 同时写入 append-only event log：

```text
job.created
job.started
item.succeeded
item.failed
job.succeeded
callback.delivered
```

用途：

- Web UI 展示任务时间线。
- CLI 查询最近事件。
- webhook 失败时，发布方仍可通过轮询发现终态。
- 后续实现 SSE / WebSocket 实时刷新。

CLI 查询事件：

```text
douyin events --job-id job_...
```

HTTP API：

```text
GET /v1/events?job_id=job_...
```

通知语义上，webhook 是主动提醒，event log 是最终可信记录。

## Web 模块

需要增加一个只面向本地用户和 Agent 运维的 Web 模块，默认由 daemon 提供。

建议命令：

```text
douyin serve --bind 127.0.0.1:8787
```

Web 模块包含两部分：

- JSON API：给 CLI、Agent 包装层和前端页面使用。
- 静态页面：给用户查看任务状态、错误、产物和 Cookie/network 健康情况。

默认安全策略：

- 默认只监听 `127.0.0.1`。
- 不展示 Cookie 原文。
- 不展示完整敏感 Header。
- artifact 下载 URL 默认本机可访问。
- 如需监听 `0.0.0.0`，必须显式配置 token。

页面建议：

- Jobs：任务列表，按 `queued/running/succeeded/partial/failed/cancelled` 过滤。
- Job Detail：单任务进度、item 明细、错误、事件时间线、artifact 列表。
- Artifacts：已下载文件、大小、hash、本地路径、HTTP 下载入口。
- Credentials：Cookie 健康状态、过期提示、最近验证结果。
- Network Profiles：出口健康、限流/封禁状态、冷却时间。
- Callbacks：通知发送状态、失败重试记录。

前端可以先做成极简静态页面，不必引入复杂前端框架。第一版只需要 HTML + 少量 JS 轮询：

```text
GET /api/jobs
GET /api/jobs/{job_id}
GET /api/events?job_id=...
```

如果要在 Rust 内实现 Web server，需要新增依赖。候选：

- `axum`：生态成熟，适合 JSON API + 静态资源。
- `poem`：API 设计直接，也适合小服务。
- `tiny_http` 或 `rouille`：依赖更轻，但异步和生态弱一些。

本项目已经使用 `tokio` 和 `reqwest`，若用户同意新增依赖，优先建议 `axum` + `tower-http`。如果暂时不新增依赖，可以先把 Web 模块作为设计保留，或用外部静态页面读取 daemon API。

## 后续阶段

1. 下载写入改为 `.partial` + atomic rename。
2. status 增加 `error_kind`、`attempt`、`heartbeat_at`。
3. 增加 `retry` / `cancel` 命令。
4. 将 Cookie 从文件路径升级为 credential id。
5. 将输出文件注册到通用 artifact-store。
6. 将 worker 模式升级为可选 daemon，并接入 `custom-utils` daemon feature。
7. 增加本地 HTTP API，CLI 通过 API 与 daemon 通信。
8. 增加 callback 持久队列和 event log。
9. 增加 Web 模块，提供任务、事件、artifact、credential、network profile 查看页面。
