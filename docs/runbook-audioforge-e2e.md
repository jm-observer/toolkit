# Runbook：英语音频生产线端到端验收（Phase 3 / 流 B）

从**一段专题文本**到 **english 中可见的学习包**，全程零人工传文件：
`文本句子清单 → AudioForge 逐句 TTS → 学习包草稿(manifest+wav) → english package.import 落库`。
供人工真机验收用。

> 平台：Windows 11 / PowerShell。真机需 **toolkit-server + 上游 TTS(CosyVoice2) + english 后端 + MySQL**。
> 本地纯逻辑验证见 `cargo test -p toolkit-server --test audio_forge`（mock 上游 TTS，不需真实依赖）。

## 0. 前置依赖

| 依赖 | 说明 | 健康检查 |
|---|---|---|
| toolkit-server | 本仓库 `cargo run -p toolkit-server -- serve` | `GET /api/web/health` |
| 上游 TTS | CosyVoice2 FastAPI（`POST /tts` 回 WAV） | `GET /api/web/audio/voices` |
| english 后端 | `D:\git\english`，`cargo run -- -w <ws>` | `GET /health` |
| MySQL | english 元数据库 | english 启动日志 |

## 1. 配置环境变量

toolkit-server 侧：

```powershell
$env:TOOLKIT_WORKSPACE = "D:\toolkit-data"        # 持久状态根（含 audioforge/）
$env:TTS_BASE_URL       = "http://127.0.0.1:8095" # 上游 CosyVoice2（复用 Phase 1 约定）
```

> 未设 `TTS_BASE_URL` 时，`audio_forge` 任务提交后**立即 failed** 并在 `error` 里说明
> （不会空跑落盘）。

english 侧（`<english-workspace>/.env`，形态 B 调用用得到）：

```
TOOLKIT_BASE_URL=http://127.0.0.1:8788
```

## 2. 准备专题句子（来源 2：LLM 直接按专题生成）

现阶段用 LLM（任意聊天界面）按专题生成句子清单。示例提示词：

```
你是英语学习内容编辑。请就「数字」专题生成 8 个适合初学者跟读的英文短句，
每句给出中文译文。只输出 JSON 数组，每项形如：
{"text": "<英文句子>", "translation": "<中文译文>"}
不要输出任何额外说明。
```

把模型输出的数组贴进下一步的 `sentences` 字段。

> 来源 1（抖音整理稿抽句）：用 `from_refined: {aweme_ids: [...]}` 替代 `sentences`，
> 从 `<workspace>/douyin/refined/<id>.json` 抽句。**当前为整理稿全文按句切分的简化实现**，
> 英语片段精选逻辑待迭代。

## 3. 提交 AudioForge（toolkit）

```powershell
$forge = @{
  package_name = "数字入门"
  topic        = "数字"
  language     = "en"
  voice_id     = "edge_en_female"        # GET /api/web/audio/voices 里挑一个
  tts_params   = @{ speed = 1.0 }         # 可选：语速 / instruct 等，平铺给上游
  sentences    = @(
    @{ text = "One."; translation = "一" },
    @{ text = "Two."; translation = "二" }
    # ... 贴入第 2 步生成的句子
  )
} | ConvertTo-Json -Depth 6

$resp = Invoke-RestMethod -Method Post -Uri http://127.0.0.1:8788/api/web/audio/forge `
  -ContentType "application/json" -Body $forge
$task = $resp.task_id
$task
```

轮询任务直到终态，拿到 `package_id` 与 `manifest_url`：

```powershell
do {
  Start-Sleep -Seconds 1
  $st = Invoke-RestMethod http://127.0.0.1:8788/api/web/tasks/$task
  $st.state
} while ($st.state -in @("queued","running"))
$st.output       # { package_id, generated, failed, manifest_url, failures[] ... }
$pkg = $st.output.package_id
```

产物落 `<workspace>/audioforge/<package_id>/`：`manifest.json` + `001.wav`、`002.wav`…
可直接访问校验：

```powershell
Invoke-RestMethod http://127.0.0.1:8788/api/web/audio/forge/$pkg/manifest.json
# 音频：http://127.0.0.1:8788/api/web/audio/forge/$pkg/001.wav
```

## 4. 登录 english 拿鉴权 token

```powershell
$login = @{ method = "auth.login"; params = @{ username = "admin"; password = "<pwd>" } } | ConvertTo-Json
$lr = Invoke-RestMethod -Method Post -Uri http://127.0.0.1:28080/api `
  -ContentType "application/json" -Body $login
$token = $lr.data.token       # 字段名以实际登录返回为准
```

## 5. 导入到 english（package.import）

形态 A（直接给 manifest URL）：

```powershell
$imp = @{
  method = "package.import"
  params = @{ manifest_url = "http://127.0.0.1:8788/api/web/audio/forge/$pkg/manifest.json" }
} | ConvertTo-Json -Depth 4

Invoke-RestMethod -Method Post -Uri http://127.0.0.1:28080/api/auth `
  -Headers @{ Authorization = "Bearer $token" } `
  -ContentType "application/json" -Body $imp
```

形态 B（给 package_id，base 读 english 的 `TOOLKIT_BASE_URL`）：

```powershell
$imp = @{ method = "package.import"; params = @{ package_id = "$pkg" } } | ConvertTo-Json
# 同上 POST /api/auth
```

返回 `data` 含 `package_id`（english 侧自增 id）、`imported_sentences`、`imported_audios` 等。

## 6. 验收

- english 管理端 / 小程序能看到名为「数字入门」的学习包，句子可播放。
- `audio_files` 记录指向 `<english-workspace>/audio/NNN.wav`，`GET /audio/{audio_id}` 可播。
- 学习包 `packages.source = 'toolkit'`。

## 故障排查

| 现象 | 排查 |
|---|---|
| forge 任务立即 failed，error 含 `TTS_BASE_URL` | toolkit 未配 `TTS_BASE_URL` |
| forge output `failures[]` 非空 | 上游 TTS 对该句多次失败（已重试 3 次）；看 error 文本 |
| package.import 报拉取 manifest 失败 | toolkit 不可达 / package_id 错 / 形态 B 未配 `TOOLKIT_BASE_URL` |
| 句子有但无音频 | `failed_audios>0`：音频 URL 拉取失败；句子已落库，可重导补音频 |
| 导入报鉴权失败 | token 过期 / 未带 Authorization 头 |

## 已知遗留

- manifest 的 `translation` / `note` 当前**未落 english 库**（`sentences` 表无对应列），见
  `english/docs/2026-06-11-toolkit-package-import/`。
- 重复导入同一 toolkit 包会在 english 新建包（句子按 text 去重，包不去重）。
