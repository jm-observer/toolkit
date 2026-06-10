# Runbook：抖音知识管线端到端验收（Phase 2）

从零跑通**一个博主**的完整流 A：`download → ASR → TextRefine → kb_publish → rag ingest`，
最终用 RAG 检索到整理后的内容。供人工端到端验收用。

> 平台：Windows 11 / PowerShell。真实环境需 **G10 vLLM + asr-server + 有效 cookie + embedding 服务**。
> 本地纯逻辑验证见 `cargo test -p toolkit-server`（用 mock LLM，不需真实依赖）。

## 0. 前置依赖

| 依赖 | 说明 | 健康检查 |
|---|---|---|
| toolkit-server | 本仓库 `cargo run -p toolkit-server -- serve` | `GET /api/web/health` |
| asr-server | OpenAI 兼容 ASR（sherpa-onnx），同机 | `GET :8091/healthz` |
| GB10 vLLM | OpenAI 兼容 chat completions（整理用） | `GET {LLM_BASE_URL}/models` |
| embedding 服务 | rag ingest 用（如 bge-m3，OpenAI 兼容 embeddings） | `POST {endpoint}` |
| 抖音 cookie | `<workspace>/douyin/cookies.json`（desktop 登录窗采集后上传） | `GET /api/web/douyin/cookie_status` |

## 1. 配置环境变量（TextRefine 的 LLM 连接）

```powershell
$env:TOOLKIT_WORKSPACE = "D:\toolkit-data"          # 持久状态根
$env:LLM_BASE_URL       = "http://gb10:8000/v1"     # GB10 vLLM OpenAI 兼容 base
$env:LLM_MODEL          = "Qwen2.5-7B-Instruct"     # 常驻模型名
# $env:LLM_API_KEY      = "..."                      # vLLM 默认无鉴权时不设
# $env:RAG_BIN          = "D:\path\to\rag.exe"       # 缺省取 toolkit-server 同目录下的 rag
```

> 未设 `LLM_BASE_URL` / `LLM_MODEL` 时，`refine` / 含 refine 的 `pipeline` 提交后立即 failed
> 并在 `error` 里说明缺哪个变量（不会空跑下载/ASR）。

## 2. 准备 rag 配置文件

`<workspace>/rag-config.json`（embedding endpoint 改成你的）：

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

## 3. 起 toolkit-server

```powershell
cargo run -p toolkit-server -- serve --workspace $env:TOOLKIT_WORKSPACE --bind 127.0.0.1:8788
# 另开一个终端做下面的调用
$base = "http://127.0.0.1:8788"
```

健康检查：

```powershell
irm "$base/api/web/health"
irm "$base/api/web/douyin/cookie_status"   # has_required / logged_in 应为 true
```

## 4A. 一键编排（推荐）

`POST /api/web/douyin/pipeline` 串联全链路。首次需 `sync_works: true` 拉作品缓存：

```powershell
$body = @{
  handle    = "https://www.douyin.com/user/MS4wLongSecUid..."  # 或裸 unique_id
  tags      = @("数字")          # 标签筛选；留空 @() 表示全部已缓存作品
  match_all = $false
  max_pages = 60
  stages    = @{
    sync_works = $true           # 首次必开：拉作品列表落缓存
    download   = $true
    transcribe = $true
    refine     = $true
    kb_publish = $true
    rag_ingest = $true
  }
  rag_config = "$env:TOOLKIT_WORKSPACE\rag-config.json"
} | ConvertTo-Json -Depth 5

$r = irm -Method Post "$base/api/web/douyin/pipeline" -ContentType "application/json" -Body $body
$taskId = $r.task_id
```

轮询进度（聚合了各阶段）：

```powershell
while ($true) {
  $t = irm "$base/api/web/tasks/$taskId"
  "{0}  stage={1} {2}/{3}" -f $t.state, $t.progress.stage, $t.progress.stage_index, $t.progress.stage_total
  if ($t.state -notin @("queued","running")) { $t | ConvertTo-Json -Depth 6; break }
  Start-Sleep 2
}
```

**预期产物**（`<workspace>` 下）：

- `downloads/douyin/<aweme_id>.mp4` —— 无水印视频
- `douyin/transcripts/<aweme_id>.json` —— ASR 原文（`text` + 可选 `segments`）
- `douyin/refined/<aweme_id>.json` —— 整理稿（`refined_text` + `model` + `prompt_version` + `prompt_hash` + `refined_at`）
- `knowledge/douyin/<unique_id>/transcripts/<aweme_id>.md` —— md 含 `has_refined: true`、`## 整理稿（LLM）`（原文栏仍保留）
- `knowledge/douyin/<unique_id>/{profile.md,index.md}`
- `rag.db` —— sqlite-vec，整理稿已 upsert

## 4B. 分步执行（排障时逐环节验证）

```powershell
# 1) 拉作品列表（落 works 缓存，供 tags/filter/pipeline 用）
irm -Method Post "$base/api/web/douyin/sync_works" -ContentType application/json -Body (@{handle="<url或unique_id>"; max_pages=60} | ConvertTo-Json)
# 看标签
irm "$base/api/web/douyin/tags?unique_id=<unique_id>"
# 按标签筛 aweme_ids
$ids = (irm "$base/api/web/douyin/filter?unique_id=<unique_id>&tags=数字&match=any").aweme_ids

# 2) 下载 + ASR（process 合并任务）
irm -Method Post "$base/api/web/douyin/transcribe" -ContentType application/json -Body (@{aweme_ids=$ids; unique_id="<unique_id>"} | ConvertTo-Json)

# 3) LLM 整理（显式 id，或留空整理「全部已转写未整理」）
irm -Method Post "$base/api/web/douyin/refine" -ContentType application/json -Body (@{aweme_ids=$ids; unique_id="<unique_id>"} | ConvertTo-Json)

# 4) 写知识包（回填原文 + 整理稿）
irm -Method Post "$base/api/web/douyin/kb_publish" -ContentType application/json -Body (@{unique_id="<unique_id>"; only_ids=$ids} | ConvertTo-Json)

# 5) RAG 录入
& rag ingest --config "$env:TOOLKIT_WORKSPACE\rag-config.json" --workspace $env:TOOLKIT_WORKSPACE
```

## 5. 验收：RAG 检索到整理后的内容

```powershell
& rag search --config "$env:TOOLKIT_WORKSPACE\rag-config.json" --workspace $env:TOOLKIT_WORKSPACE --query "数字怎么读" --top_k 5
```

预期：命中条目的文本来自 `## 整理稿（LLM）` 段（整理稿置于 md 前部，优先被检索），
`metadata.source_path` 指向 `knowledge/douyin/<unique_id>/transcripts/<aweme_id>.md`。

## 6. 迭代整理 prompt

- prompt 文本：`crates/douyin/src/refine_prompt.md`（`{TRANSCRIPT}` 占位符）。
- 改 prompt 后 bump `crates/douyin/src/refine.rs` 的 `PROMPT_VERSION`。
- 重跑：删对应 `douyin/refined/<id>.json`（或全清 `douyin/refined/`）后再 `refine` →
  新整理稿元信息里 `prompt_hash` / `prompt_version` 变化，可与旧产物对比。

## 7. 失败排查

| 现象 | 排查 |
|---|---|
| refine 任务一提交就 failed，error 提到 LLM_BASE_URL/LLM_MODEL | 环境变量没设到 toolkit-server 进程里 |
| refine output `failures[]` 有条目 | 看 error：`无 ASR 转写缓存`=该 id 没先 transcribe；`LLM xxx`=上游报错（已重试 3 次） |
| pipeline 在 resolve 阶段 failed | 作品缓存缺失 → 开 `sync_works: true` 或先单独 `sync_works` |
| rag_ingest failed `spawn rag` | rag 二进制不在 toolkit-server 同目录；设 `RAG_BIN` 指向它 |
| rag search 无结果 | 确认 embedding 服务在线、`dim` 与 `rag.db` 一致；确认 kb_publish 已写 md |
