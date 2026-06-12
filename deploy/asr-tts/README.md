# deploy/asr-tts — 语音底座编排（仅 TTS，G10）

> 历史：该目录原本同时编排 **ASR**（本仓 `crates/asr-server`，sherpa-onnx）+ **TTS**
> （CosyVoice2）。2026-06 起，ASR 统一由 **streaming-speech** 仓的 FunASR 服务提供
> （Paraformer/SenseVoice/Whisper GPU 全套 + 声纹门控 + 实时流式管线 + 离线
> `/transcribe` HTTP 端点），sherpa-onnx asr-server 已物理退役（crate / Dockerfile /
> Bin 全部删除）。本编排只保留 **TTS** 一项。

toolkit 与 streaming-speech **同机部署**在 G10/GB10 (192.168.0.68)。

## 拓扑

| 服务 | 端口（仅本机） | 引擎 | 镜像 | 维护仓 |
|---|---|---|---|---|
| FunASR | `127.0.0.1:9101` (`/transcribe` + `/embed`) | Paraformer / SenseVoice / Whisper (GPU) | `funasr-asr:svc` | **streaming-speech** `server/asr` |
| tts | `127.0.0.1:8095` | CosyVoice2-0.5B (GPU) | `cosyvoice2:bakeoff` | streaming-speech `server/tts`（本编排引用） |

两端口都只发布到 `127.0.0.1`：
- **FunASR /transcribe** 由同机的 douyin process 任务 multipart 上传 mp4 字节调
  （`crates/douyin/src/process.rs`）。FunASR 服务本身随 streaming-speech 部署
  （`scripts/release-server.ps1`），不由本编排管理。
- **TTS** 对外统一走 toolkit-server 的 `/api/web/audio/tts` 代理，不直接暴露。

## 宿主资产准备（不进镜像 / 不进 git）

| 路径（默认，可用 compose 环境变量覆盖） | 内容 | 来源 |
|---|---|---|
| `~/funasr-prep/models/CosyVoice2-0.5B/` | CosyVoice2 权重（~5GB） | modelscope `iic/CosyVoice2-0.5B` |
| `~/tts-voices/` | 音色库 `voices.json` + `*.wav` | 见 streaming-speech `server/tts/README.md` |
| `~/tts-io/` | TTS 输入/输出工作目录（可空） | 运行期使用 |

可覆盖的环境变量（写进 `.env` 或 export）：
`TTS_MODELS_DIR` / `TTS_IO` / `TTS_VOICES`。

## 镜像准备

复用 streaming-speech 已构建的 `cosyvoice2:bakeoff` 镜像。G10 若没有，按
`streaming-speech/server/tts/README.md` 在 G10 构建好该镜像（arm64+CUDA13，~10-15min），
本编排不重复其构建定义，仅引用镜像。

## 运行

```bash
docker compose -f compose.yaml up -d

# 冒烟
curl http://127.0.0.1:8095/health                        # tts → {"ok":true,...}
# FunASR /transcribe 不由本编排管理,smoke 见 streaming-speech docs/DEPLOYMENT.md
```

`restart: unless-stopped`：G10 重启自动拉回。

## 与 toolkit-server 的对接

- TTS：toolkit-server 起进程时设 `TTS_BASE_URL=http://127.0.0.1:8095`。
- ASR：douyin 任务的默认 `asr_url` 已硬编码为 `http://127.0.0.1:9101/transcribe`
  （`crates/douyin/src/lib.rs` + `main.rs` + `crates/toolkit-server/src/douyin/*`）。
