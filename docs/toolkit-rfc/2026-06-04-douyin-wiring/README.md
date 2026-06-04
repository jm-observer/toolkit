# RFC：抖音业务接线（2026-06-04，Plan 2）

> 把现有 `crates/douyin` 业务函数接到 `toolkit-server` 的 HTTP 路由上；其中 3 个长任务（download / process / list-works）包装成 toolkit-tasks 的 TaskKind，统一通过 SQLite tasks 表观察。

## 时间

- 创建：2026-06-04
- 状态：已完成（2026-06-04）

## 前置依赖

- Plan 1（toolkit-core / toolkit-tasks / toolkit-server 骨架）已完成

## 设计关键

### 一、路径桥接（toolkit data_dir → douyin 既定布局）

douyin crate 里所有路径都接受显式 `Path`，没有路径就回退到 `$ZERO_WORKSPACE/...`。toolkit-server 永远**传显式路径**，避免依赖 ZERO_WORKSPACE：

| douyin 概念 | toolkit-server 解析为 |
|---|---|
| `cookie_file` | `<data_dir>/douyin/cookies.json` |
| `task_dir` | `<data_dir>/douyin/tasks` |
| `out_dir` | `<data_dir>/downloads/douyin` |
| `transcript_dir` | `<data_dir>/douyin/transcripts` |
| `works_dir` | `<data_dir>/douyin/works` |
| `knowledge_dir` | `<data_dir>/knowledge/douyin` |

封装在 `toolkit-server/src/douyin/paths.rs`，提供 `DouyinPaths::new(data_dir)`。

### 二、Cookie 桥接

`/api/browser/cookie` 已经在 Plan 1 写入 `cookies` 表。**新增**：同步写一份 douyin 兼容格式到 `<data_dir>/douyin/cookies.json`，调 `douyin::run_set_cookie`（它接受 raw_header）。失败仅警告日志，不阻断 endpoint。

### 三、Task kind 包装

3 个 douyin 异步 submit 都是「立刻返回 douyin task_id，worker 后台跑，状态在 status.json」的模式。toolkit-tasks 包装做法：

```
TaskKind::run(input):
  1. 调 douyin::run_*_submit → 拿 douyin_task_id（保存到 ctx 的 progress 里）
  2. 循环 poll douyin::*_read_status，state 进 terminal 退出
  3. 把 douyin 终态 status 整体作为 Output 返回
失败：douyin 返回 error/state=failed → run 返回 Err
```

3 个 kind：

| KIND | 输入 | 包装的 douyin 调用 |
|---|---|---|
| `douyin_download` | `{aweme_ids, out_dir_override?}` | `run_download_submit` + `download::read_status` 轮询 |
| `douyin_transcribe` | `{aweme_ids, vad?, asr_url?, asr_model?, unique_id?}` | `run_process_submit` + `process::read_status` 轮询 |
| `douyin_list_works` | `{handle, max_pages?}` | `run_list_works_submit` + `list_works_task::read_status` 轮询 |

`douyin_kb_publish` 是 sync 函数，不通过 TaskKind 包装——直接在 HTTP handler 里同步调，几百 ms 内返回。

> **轮询间隔**：固定 2s，没有指数退避（douyin 内部 worker 是真实速率，2s 够看进度）。

### 四、HTTP 路由（/api/web/douyin/*）

| 方法 | 路径 | 处理 |
|---|---|---|
| GET | `/api/web/douyin/creator?handle=...` | `run_resolve_user`，直接返回 douyin JSON |
| GET | `/api/web/douyin/works?handle=...&max_pages=...` | `run_list_works`（同步，慢但简单） |
| GET | `/api/web/douyin/tags?unique_id=...` | `run_list_tags` |
| GET | `/api/web/douyin/filter?unique_id=...&tags=A,B&match=all` | `run_filter_works` |
| POST | `/api/web/douyin/sync_works` body `{handle, max_pages?}` | submit `douyin_list_works` task，返 `{task_id}` |
| POST | `/api/web/douyin/download` body `{aweme_ids}` | submit `douyin_download` task |
| POST | `/api/web/douyin/transcribe` body `{aweme_ids, vad?, unique_id?}` | submit `douyin_transcribe` task |
| POST | `/api/web/douyin/kb_publish` body `{unique_id, only_ids?}` | sync 调 `run_publish_knowledge`，直接返结果 |
| GET | `/api/web/douyin/cookie_status` | `run_cookie_status`（自检 + login 实测） |

任务进度查询统一走 Plan 1 的 `GET /api/web/tasks/{tk_id}`——progress 列里能看到 douyin_task_id 与 douyin 内部进度。

### 五、不在 Plan 2 范围

- 入库 SQLite creators / works 表（Plan 3 做，跟 UI 一起设计字段映射）
- Agent namespace 粗粒度入口（Plan 5）
- 任务取消 / 重试 endpoint（先让 toolkit-tasks 统一支持，本 Plan 不做）
- 浏览器扩展本体（Plan 4）

### 六、测试边界

douyin 业务函数大量依赖真实 cookies + 抖音 HTTP API，无法单测。Plan 2 测试：
- 单测：DouyinPaths 路径解析、cookie 桥接函数（无网络）
- 单测：3 个 TaskKind 注册存在、未注册 kind 报错
- HTTP 路由 smoke 测：endpoint 存在 + 入参错误 400（不打抖音 API）
- 真机验证（用户操作）：手动启 server + 真 cookies + 一个真博主，跑端到端

## 风险

- **douyin 内部 worker 跑在哪个 runtime**：`run_*_submit` 在内部 `tokio::spawn`，挂当前 runtime。toolkit-server 是 multi-thread runtime，没问题。
- **TaskKind run 长时间持有 tokio task slot**：3 个长任务并行最多 3 个 slot，可接受。
- **轮询期间 toolkit-server 崩了**：toolkit 任务标 interrupted（Plan 1 恢复逻辑），douyin 内部任务仍在 status.json 里。后续手动 re-submit 或加扫描逻辑（Plan 3+ 决策）。
