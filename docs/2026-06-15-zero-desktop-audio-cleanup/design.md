# zero-desktop 音频清洗功能设计

> 设计文档（仅设计，未实现）。日期 2026-06-15。
> 权威 API 源：streaming-speech 仓 `docs/audio-cleanup-api.md`（`POST /clean` @ `:8097`）。
> 关联：streaming-speech `docs/2026-06-14-audio-cleanup/audio-cleanup-plan-4.md`（本设计在其基础上
> 因 zero-desktop 实际架构而调整音频来源）。

---

## 1. 背景与动机

audio-cleanup 服务提供 `POST /clean`（同机 `127.0.0.1:8097`，multipart）：上传**任意 ffmpeg 可解码
音/视频**，经 Demucs 人声分离（去 BGM）/ DeepFilterNet 降噪去混响 / 停顿处理 / EBU R128 响度归一，
回传清洗后音频（wav/mp3/flac），清洗元数据放 `X-Cleanup-Stages` / `X-Cleanup-In-LUFS` /
`X-Cleanup-Out-LUFS` 响应头。

我们要在 zero-desktop 桌面端提供一个清洗入口，让用户拿到去噪/去 BGM 后的音频（给人听、或作 ASR 输入、
或作 TTS 克隆参考音）。

### 与 plan-4 的偏差（关键）

plan-4 设想「清洗**本地麦克风录音**」，但该前提在 zero-desktop 现状下**不成立**：

- zero-desktop 是**纯远程架构**：麦克风 PCM 经 `spawn_capture`
  （`crates/zero-desktop/src/modules/speech/commands/remote.rs:277`）直接流到 GB10 orchestrator
  做识别，**本地不落任何 wav 文件**（全仓除 WIP `clean.rs` 外无任何 `WavWriter` / `.wav` 写盘）。
- 因此「对一段本地录音点清洗」没有源文件可指。

**决策**：音频来源改为**文件选择器**——用户挑任意本地音/视频文件清洗。这既契合 `/clean` API
「接受任意文件」的能力，也与已有 WIP 命令 `speech_clean_recording(input_path, …)` 的签名一致
（它本就接收任意路径）。plan-4 的「本地录音落盘再清洗」分支工作量大（需改录音管线 + 新增存储），
排期为后续迭代。

### 现状盘点

- WIP 后端 `crates/zero-desktop/src/modules/speech/commands/clean.rs` 已存在：`speech_clean_recording`
  已在 `main.rs:120` 注册，接收任意 `input_path`，经代理清洗，并列落 `<stem>.cleaned.wav`，
  **不覆盖原文件**（已有安全闸 + 单测）。但：
  - 只暴露 `denoise` / `pause` 两个选项（API 还有 `separate` / `level` / `loudness` / `sr` / `format`）。
  - 输出扩展名硬编码 `.cleaned.wav`，未跟随 `format`。
  - 依赖一条 **尚不存在** 的代理路由 `POST /api/web/audio/clean`。
  - MIME 硬编码 `audio/wav`，与「任意音/视频」目标矛盾（见 §3.0）。
  - 无前端入口。（toolkit-server base 复用全局 `g10_base`，无需新增配置，见 §5.1。）

---

## 2. 架构与数据流

延续「桌面端不直连 GB10，统一经 toolkit-server `:8788` 代理」的既有先例（TTS 走
`/api/web/audio/tts`，见 `crates/toolkit-server/src/routes/audio.rs`）。

```
用户（文件选择器选 音/视频 文件）
  → speech_clean_recording        (Tauri command，reqwest multipart 上传)
  → toolkit-server POST /api/web/audio/clean   (:8788 代理，本设计的依赖，属 plan-3)
  → 上游 audio-cleanup POST /clean             (:8097，由 CLEAN_BASE_URL 指向)
  → 回传 音频 body + X-Cleanup-{Stages,In-LUFS,Out-LUFS} 响应头
  → 桌面端落 <stem>.cleaned.<format> 并列（原文件字节不动）
  → 前端展示 cleaned 路径 + stages + in/out LUFS + 「打开所在文件夹」
```

---

## 3. 后端命令设计（扩展现有 `clean.rs`，不新建文件）

### 3.-1 命令签名：后端自取配置 + Bearer token（定稿）

**评审修订（定稿一种方案）**：命令**不再**从前端接 `toolkit_base`。改为接
`state: State<'_, AppState>`，后端自取全局配置——与 `english_get_g10_base`（`english/mod.rs:25`）
同款：`load_app_settings(&state.workspace)` 拿 `AppSettings`，再:

- 拼 endpoint：`{g10_base}/api/web/audio/clean`（或用 §5.1 的 `clean_endpoint()` helper）；
  `g10_base` 空 → 返回「G10 base 未配置」（仿 `is_configured()`，`shared/settings.rs:33`）。
- **带 Bearer token**：若 `g10_token` 非空则 `req.bearer_auth(tok)`——**与 cookie 请求同款**
  （`cookie/mod.rs:173`）。**这是原设计漏掉的**：G10/toolkit-server 开鉴权时不带 token 会稳定 401/403。

新签名（去掉 `toolkit_base`，加 `state`）：

```rust
#[tauri::command]
pub async fn speech_clean_recording(
    state: tauri::State<'_, AppState>,
    input_path: String,
    denoise: Option<bool>, pause: Option<String>, separate: Option<bool>,
    level: Option<String>, loudness: Option<String>, sr: Option<u32>, format: Option<String>,
) -> Result<CleanedRecording, String>
```

前端只传 `input_path` + 选项，**不碰** base/token（配置细节不外泄到前端）。

### 3.0 multipart 文件 part：字段名与 MIME

权威 API（`audio-cleanup-api.md` §请求）规定文件字段名是 **`audio`**（不是 `file`），WIP 现状
`.part("audio", …)`（`clean.rs:71`）已正确，保持。

但 WIP 把 MIME **硬编码** `audio/wav`（`clean.rs:73`），与本设计「接受任意音/视频」目标矛盾——对
mp4/webm/m4a 传 `audio/wav` 是错误标注。处理：

- 服务端用 ffmpeg **按内容**解码（视频自动抽音轨），MIME 仅为提示、不决定解码路径，但仍应如实标注。
- **按扩展名映射** MIME（小表：wav→`audio/wav`、mp3→`audio/mpeg`、m4a→`audio/mp4`、flac→`audio/flac`、
  mp4→`video/mp4`、webm→`video/webm`），**未知扩展名回退 `application/octet-stream`**。
- 文件名（`Part::file_name`）保留真实文件名（含原扩展名），便于服务端/日志辨识。

### 3.1 入参对齐 API 完整选项集

现签名只有 `denoise` / `pause`。扩展为（全部 `Option`，缺省则**不发**该 form 字段，让上游用其默认，
保持向后兼容）：

| 命令入参 | 类型 | 对应 form 字段 | 上游默认 | 说明 |
|---|---|---|---|---|
| `separate` | `Option<bool>` | `separate` | `0` | Demucs 去 BGM 取 vocals（慢）。 |
| `denoise` | `Option<bool>` | `denoise` | `1` | DeepFilterNet 降噪去混响。 |
| `pause` | `Option<String>` | `pause` | `duck` | `drop` 删非语音 / `duck` 压低 / `off` 不动。 |
| `level` | `Option<String>` | `level` | `balanced` | `gentle`/`balanced`/`aggressive`。 |
| `loudness` | `Option<String>` | `loudness` | `-16` | 目标 LUFS 或 `off`。 |
| `sr` | `Option<u32>` | `sr` | `48000` | 输出采样率 16000/24000/48000。 |
| `format` | `Option<String>` | `format` | `wav` | `wav`/`mp3`/`flac`。 |

`bool` → form 取 `"1"`/`"0"`（沿用现有 `denoise` 写法）。建议命令入口对 `pause`/`level`/`format`/`sr`
做白名单校验（非法值早返回可读错误，避免无谓上传）。

### 3.2 输出扩展名跟随 `format`

`cleaned_variant_path`（`clean.rs:30`）当前硬编码 `.cleaned.wav`。改为按 `format` 决定后缀：
`<stem>.cleaned.<format>`（默认 `wav`）。**保留**现有安全闸——派生路径必不等于原路径，
落盘前再断言 `out_path != input` 以防覆盖（已有逻辑，`clean.rs:122`）。

### 3.3 错误映射（透传、不静默吞，对齐 API 错误表）

| 来源状态 | 桌面端提示（中文，可读） |
|---|---|
| 代理 503（`CLEAN_BASE_URL` 未配） | 「清洗服务未配置（toolkit-server 缺 CLEAN_BASE_URL）」 |
| 代理 502（上游不可达） | 「清洗服务不可达（上游 audio-cleanup 无响应）」 |
| 401 / 403（鉴权） | 「鉴权失败，请检查 G10 token 配置」（见 §3.-1，请求已带 Bearer） |
| 上游 400 | 「文件无法解码或字段错误」 |
| 上游 413 | 「文件过大，请先转码/截取」 |
| 上游 422 | 「音频时长超限，请切分后再传」 |
| 上游 503（队列满 busy） | 「清洗服务繁忙，请稍后重试」 |
| 上游 504 | 「处理超时（超 600s），请切分输入」 |
| 上游 500 | 「清洗服务内部错误」 |

**设计要点 —— 503 语义冲突**：代理用 503 表示「未配置」，而上游 audio-cleanup **也**用 503 表示
「队列满 busy」。两者提示完全不同（未配置是部署问题、busy 是临时重试）。要求**代理层**（plan-3）
区分二者，约定见 §6：
- 未配置：代理回 503，body `{"error": "clean upstream not configured", ...}` 且带专用响应头
  `X-Clean-Proxy: unconfigured`。
- 上游 busy：代理**透传**上游 503，**不带** `X-Clean-Proxy: unconfigured`。

桌面端据 `X-Clean-Proxy` 头（或 error body 文案）区分 → 给「未配置」vs「繁忙重试」两种提示。

### 3.4 返回结构（不变）

```rust
struct CleanedRecording {
    cleaned_path: String,   // 并列落盘路径 <stem>.cleaned.<format>
    stages: Vec<String>,    // 取自 X-Cleanup-Stages
    in_lufs: f32,           // 取自 X-Cleanup-In-LUFS
    out_lufs: f32,          // 取自 X-Cleanup-Out-LUFS
}
```

### 3.5 超时

命令侧 `CLIENT_TIMEOUT`（现 `CLEAN_TIMEOUT = 600s`，`clean.rs:17`）已对齐上游 `MAX_DURATION_SEC=600`，
保持不变。

---

## 4. 录音库关联记录 —— MVP 不入库

plan-4 提「录音库新增一条关联记录」。但 zero-desktop 现 DB（`crates/zero-desktop/migrations/`）是
**segment 为中心**（识别分段 + LLM 结果），无「录音文件」实体；文件选择器来源的外部文件与该库
无天然关联。

**取舍**：MVP **不入库**，命令直接返回 `CleanedRecording` 供前端即时展示即可。

**可选迭代**：若日后要历史留痕/复用，再加轻量表：

```
cleaned_audio(
  id INTEGER PK,
  source_path TEXT,
  cleaned_path TEXT,
  stages TEXT,           -- 逗号分隔
  in_lufs REAL,
  out_lufs REAL,
  created_at TEXT
)
```
（走现有幂等迁移风格，新增一个 `migrations/000X_*.sql`。）

---

## 5. 前端设计

### 5.1 toolkit-server base URL —— 复用全局 `g10_base`，不新增配置

**评审修订**：原设计拟在 speech DB 另存 `toolkit.base_url`，会与**全局** `g10_base` 重复。
项目已有全局 `AppSettings.g10_base`（`shared/settings.rs:13`，落盘 `{workspace}/app.json`，注释即
「G10 server base 例如 `http://...:8788`」），**cookie 模块与 english 模块都共享它**
（cookie 经 `cookie_get_app_settings`、english 经 `english_get_g10_base`，设置页统一维护）。
若再存一份，用户要维护两个指向同一 toolkit-server 的 base URL，cookie/english 与 cleanup 可能打到
**不同服务器**。

因此 cleanup **直接复用全局 `g10_base` + `g10_token`**，且**由后端自取**（见 §3.-1 定稿签名）：

- 不新增任何 speech 设置项，不动 `CombinedSettings` / `remote_url`（`remote_url` 是 `ws://` 的 ASR
  orchestrator 地址，本就与此无关）。
- 后端 `load_app_settings(&state.workspace)` 取 `g10_base` + `g10_token`；可仿
  `cookie_endpoint()`（`shared/settings.rs:37`）加 `clean_endpoint()` helper 拼
  `{g10_base}/api/web/audio/clean`，并按 cookie 同款加 Bearer token。
- **前端不传 base/token**（旧设计的「前端读 g10_base 再传入」方案已废弃，统一为后端自取——一处实现、
  顺带解决 token、不外泄配置）。设置页无需新增输入框——`g10_base`/`g10_token` 已在那里维护。
- 注意默认值：`g10_base` 默认是 `https://www.for-memory.cloud:28080`（非 localhost）；cleanup 沿用同一
  默认，符合「与 cookie/english 同源」预期。

### 5.2 入口位置

`SpeechPage.tsx` 新增一个独立「音频清洗」卡片，与 `ControlPanel` 并列。**与 segment 识别流解耦**
——来源是任意文件而非当前会话录音，不混进 segment 列表。

### 5.3 交互流程

1. 点「选择文件」→ 选音/视频文件（wav/mp3/m4a/flac/mp4/webm…）。
   - **依赖/规约修订**：Rust 侧 `tauri-plugin-dialog` 已在 `Cargo.toml:14`，capability `dialog:default`
     已在 `capabilities/default.json:43`；但**前端 npm 包 `@tauri-apps/plugin-dialog` 不在
     `ui/package.json`**。仓库规约「未经用户明确同意不得加新依赖」——故：
     - **首选（无新增前端依赖）**：新增**后端命令**用已有 `tauri_plugin_dialog` Rust API 弹文件选择，
       返回路径给前端。
     - 备选：新增前端 `@tauri-apps/plugin-dialog` 调 `open()`——**需先取得用户对该 npm 依赖的明确同意**。
2. 选项面板：denoise（默认开）/ pause（默认 duck）/ separate（默认关）/ level（默认 balanced）/
   loudness（默认 -16）/ sr（默认 48000）/ format（默认 wav）——默认值贴 API doc。
3. 点「清洗」→ 调 `cleanRecording(inputPath, opts)`（不传 base/token，见 §3.-1）→ 进度态 spinner
   （清洗可能数分钟，命令 timeout 已 600s；UI 提示「处理中，可能需要数分钟」）。
4. 完成 → 展示：cleaned 路径、`stages` 序列、in→out LUFS、可选「打开所在文件夹」按钮。
   - **依赖修订（「打开所在文件夹」）**：项目当前**只有 shell 插件**（`tauri-plugin-shell`，
     `Cargo.toml:15`，capability `shell:default` 已在），**无 opener 插件**，前端也没有 opener/shell
     npm 包。要实现此按钮，**二选一**：(a) 新增**后端命令**用已有 `tauri-plugin-shell` 的 Rust API
     打开目录（**不引入新依赖**，推荐）；(b) 新增 `@tauri-apps/plugin-shell` npm 包前端调用。
     **若不想加任何东西，此按钮可省略**（仅展示路径文本供用户手动定位）——标为可选。
5. 失败 → 按 §3.3 给可读 toast，区分 未配置 / 不可达 / 繁忙 / 文件过大 / 时长超限 / 解码失败。

### 5.4 API 封装

`ui/src/modules/speech/api/tauri-client.ts` 新增（命名沿用 `speech_` 前缀 + camelCase wrapper 惯例）：

```ts
interface CleanOptions {
  denoise?: boolean; pause?: string; separate?: boolean;
  level?: string; loudness?: string; sr?: number; format?: string;
}
interface CleanedRecording {
  cleaned_path: string; stages: string[]; in_lufs: number; out_lufs: number;
}
cleanRecording: (inputPath: string, opts?: CleanOptions) =>
  invoke<CleanedRecording>('speech_clean_recording', { inputPath, ...opts }),
```

前端**不读也不传** base/token——后端经 `State<AppState>` 自取全局 `g10_base`/`g10_token`（§3.-1）。
前端只需在 `g10_base` 未配置时给提示（可复用已有 `cookie_get_app_settings` 判断，或由命令返回的
「G10 base 未配置」错误透出）。

---

## 6. 依赖契约：toolkit-server 代理路由（属 plan-3，另行实现）

本设计**消费**而不实现该路由；此处声明它必须满足的契约（仿 `routes/audio.rs` 的 TTS 代理）：

- **`POST /api/web/audio/clean`**：把入站 multipart **原样透传**上游 `CLEAN_BASE_URL/clean`，
  响应回传上游 body **并透传 `X-Cleanup-Stages` / `X-Cleanup-In-LUFS` / `X-Cleanup-Out-LUFS`** 头。
- `CLEAN_BASE_URL` 未配置 → `503` + `{"error":"clean upstream not configured"}` + 头
  `X-Clean-Proxy: unconfigured`（见 §3.3 区分上游 busy 的 503）。
- 上游不可达 → `502`。
- 上游其它状态码（400/413/422/503/504/500）**透传**。
- 挂载点：`routes/mod.rs` + `lib.rs` 的 `/api/web/audio` 下（与 `tts`/`voices` 同级）。
- timeout ≥ 600s（对齐上游 `CLEAN_MAX_DURATION_SEC` / `PROCESS_TIMEOUT_SEC`）。
- 可选：仿 TTS 代理加 `SpanScope` 两阶段 trace（`clean_proxy` span）。
- **可选** `GET /api/web/audio/clean/health` 代理上游 `GET /health`。

---

## 7. 文件清单（未来实现将触及）

**本 plan（zero-desktop 桌面端）**：
- `crates/zero-desktop/src/modules/speech/commands/clean.rs` — 改签名为 `State<AppState>` 自取
  `g10_base`/`g10_token`（§3.-1）、去掉 `toolkit_base` 入参、加 Bearer、扩展选项集、format 跟随后缀、
  按扩展名映射 MIME（§3.0）、错误映射、单测扩展。
- `crates/zero-desktop/src/shared/settings.rs` —（可选）加 `clean_endpoint()` helper（仿
  `cookie_endpoint()`）。**不新增** speech 设置项、**不动** `CombinedSettings`/`remote_url`。
- `crates/zero-desktop/src/modules/speech/commands/`（首选方案）— 新增**后端文件选择命令**
  用已有 `tauri_plugin_dialog` Rust API（避免新增前端 npm 依赖，§5.3）。
- `ui/src/modules/speech/api/tauri-client.ts` — `cleanRecording(inputPath, opts)` wrapper +
  `CleanOptions`/`CleanedRecording` 类型。
- `ui/src/modules/speech/SpeechPage.tsx` — 新「音频清洗」卡片。
- **依赖注意（需用户同意才动）**：若选前端文件选择/「打开所在文件夹」，要往 `ui/package.json` 加
  `@tauri-apps/plugin-dialog` /（`@tauri-apps/plugin-shell`）——按仓库规约**须先获用户明确同意**。
- **不改** `SettingsPage.tsx`（g10_base/g10_token 已在那维护）；capability 无需改动
  （`dialog:default`/`shell:default` 已具备）。

**依赖（另 plan-3，toolkit-server）**：
- `crates/toolkit-server/src/routes/audio.rs` — `clean` / `clean_health` handler。
- `crates/toolkit-server/src/routes/mod.rs` + `lib.rs` — 路由挂载。

---

## 8. 测试 / 验证（供未来实现用）

**后端单测**（沿用 `clean.rs` 现有测试风格）：
- `format=mp3` → 派生路径 `<stem>.cleaned.mp3`（扩展 `cleaned_variant_path` 测试）。
- mock 代理 200 → cleaned variant 并列落盘，且**断言原文件字节未变**。
- mock 503（含 `X-Clean-Proxy: unconfigured`）/ 502 / 上游 503 busy → 返回各自可读错误。
- 选项白名单校验：非法 `pause`/`format` → 早返回错误，不发请求。

**修复流程**（toolkit 仓根，对齐 plan-4 验收）：
```
cargo fmt --check --all
cargo clippy --workspace -- -D warnings     # 或 --exclude toolkit-desktop（无 Tauri 工具链时）
cargo test --workspace
```

**端到端手测**：
1. 起上游 audio-cleanup（`:8097`）+ toolkit-server（设 `CLEAN_BASE_URL=http://127.0.0.1:8097`）。
2. zero-desktop 设置页填**全局 `g10_base=http://127.0.0.1:8788`**（若 toolkit-server 开鉴权，
   一并填 `g10_token`）；cleanup 复用此配置，不再单独配。
3. 选一段带 BGM 的 mp4 → separate=1 / denoise=1 → 清洗 → 校验生成 `<stem>.cleaned.wav`、
   原文件仍在、`stages` 含 `separate,denoise,…`、in/out LUFS 合理（out≈-16）。
4. toolkit-server 不设 `CLEAN_BASE_URL` → 桌面端提示「清洗服务未配置」，不崩溃。
