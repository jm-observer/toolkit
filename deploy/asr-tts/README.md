# deploy/asr-tts — 语音底座编排（ASR + TTS，G10）

toolkit 提级 Phase 1 产物：把 **ASR**（本仓库 `crates/asr-server`，sherpa-onnx Rust 服务）
与 **TTS**（CosyVoice2，Python，源自 streaming-speech）放到同一份 compose，**与
toolkit-server 同机部署**在 G10。这是「TTS 与 toolkit-server 同机部署」的最小可用编排。

> 本目录只提供编排文件 + 说明；不要求从本机实际部署（GPU 服务无法从开发机验证）。

## 拓扑

| 服务 | 端口（仅本机） | 引擎 | 镜像 |
|---|---|---|---|
| asr-server | `127.0.0.1:8091` | sherpa-onnx SenseVoice int8 (CPU) | `toolkit-asr-server:latest`（本仓库构建） |
| tts | `127.0.0.1:8095` | CosyVoice2-0.5B (GPU) | `cosyvoice2:bakeoff`（streaming-speech 构建） |

两端口都只发布到 `127.0.0.1`：
- **ASR** 由同机的 douyin process 任务调（`from-source` 端点，file:// 同机路径）。
- **TTS** 对外统一走 toolkit-server 的 `/api/web/audio/tts` 代理，不直接暴露。

## 宿主资产准备（不进镜像 / 不进 git）

| 路径（默认，可用 compose 环境变量覆盖） | 内容 | 来源 |
|---|---|---|
| `<workspace>/models/sherpa-sense-voice/` | `model.int8.onnx` + `tokens.txt` | 从 streaming-speech 原 `~/asr-server-models/sherpa-sense-voice/` 迁移（模型不变），或 hf-mirror 重拉（见 `crates/asr-server/Dockerfile` 顶部命令） |
| `<workspace>/downloads/douyin/` | 抖音下载的 mp4（ASR from-source 读取目标） | 运行期由 douyin 任务写入 |
| `~/funasr-prep/models/CosyVoice2-0.5B/` | CosyVoice2 权重（~5GB） | modelscope `iic/CosyVoice2-0.5B` |
| `~/tts-voices/` | 音色库 `voices.json` + `*.wav` | 见 streaming-speech `server/tts/README.md` |

`<workspace>` 默认 `/home/fengqi/.config/toolkit-server`（与 toolkit-server 的 workspace 根一致）。

可覆盖的环境变量（写进 `.env` 或 export）：
`ASR_MODELS_DIR` / `ZERO_DOWNLOADS_DIR` / `TTS_MODELS_DIR` / `TTS_IO` / `TTS_VOICES` / `TRACE_HUB_ENDPOINT`。

## 镜像准备

- **asr-server**：`docker compose -f compose.yaml build asr-server`（本仓库 crate，
  Dockerfile 就地生成 standalone Cargo.toml 独立构建）。
- **tts**：复用 streaming-speech 已构建的 `cosyvoice2:bakeoff` 镜像。本机若没有，
  按 `streaming-speech/server/tts/README.md` 在 G10 构建好该镜像（arm64+CUDA13，~10-15min），
  本编排不重复其构建定义，仅引用镜像。

## 运行

```bash
# 起两者
docker compose -f compose.yaml up -d
# 只起其一
docker compose -f compose.yaml up -d asr-server
docker compose -f compose.yaml up -d tts

# 冒烟
curl http://127.0.0.1:8091/healthz                       # asr → ok
curl http://127.0.0.1:8095/health                        # tts → {"ok":true,...}
```

`restart: unless-stopped`：G10 重启自动拉回。

## 与 toolkit-server 的对接

toolkit-server 起进程时设环境变量：

```bash
export TTS_BASE_URL=http://127.0.0.1:8095
```

则 `/api/web/audio/tts`、`/api/web/audio/voices` 代理到本 TTS 服务。未设时这两个
端点返回 503（明确提示设 `TTS_BASE_URL`）。

douyin process 任务的 `asr_url` 默认即 `http://127.0.0.1:8091/v1/audio/transcriptions/from-source`，
天然指向本 ASR 服务，无需额外配置。

## 注意（GPU / 网络）

- TTS 需要 `runtime: nvidia` + GB10 的 arm64+CUDA13 torch 栈；CosyVoice2 首次请求
  懒加载 ~5GB 进显存（约 30s），之后稳定 < 5s/请求。
- ASR 当前 CPU-only（arm64+CUDA13 的 sherpa GPU build 是后续工作）。
- 模型/镜像都不入 git；G10 GitHub 直连不稳，模型走 hf-mirror / modelscope。
