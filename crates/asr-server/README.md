# asr-server

最小可用的 ASR HTTP 服务，OpenAI Audio API 兼容形态，基于 sherpa-onnx。
**由 `streaming-speech/server/asr-server` 整 crate 迁入 toolkit workspace**
（提级规划 Phase 1，见 `docs/toolkit-rfc/2026-06-10-toolkit-elevation/plan.md`）。
源码与端点行为与迁移前完全一致——douyin 的 `process` 任务正调它的 `from-source`
端点，迁仓不改变任何线上契约。

## 端点

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/healthz` | 存活检查，返回 `ok` |
| GET | `/v1/models` | 列出当前启动加载的模型 |
| POST | `/v1/audio/transcriptions` | 上传音频文件（multipart），返回识别文本 |
| POST | `/v1/audio/transcriptions/from-source` | 传文件路径 / HTTP URL（JSON），返回识别文本 |

`multipart` 端点接收 `file`（任意 ffmpeg 可解的音视频）+ 可选 `vad`（切段返回
`segments[]`）。`from-source` 端点接收 JSON `{source, vad?}`，`source` 形如
`file:///abs/posix/path` 或 `http(s)://…`，**仅当启动配了 `--source-allowlist` 时启用**，
否则返回 `503 endpoint_disabled`。完整请求/响应/错误形态见源码顶部注释与本 crate 的
单元测试。

## 模型文件（不入仓库）

模型体积大（SenseVoice int8 ~250MB），**不随仓库分发**，由启动参数指向磁盘路径。
约定放在 `<workspace>/models/` 下：

```
<workspace>/models/
└── sherpa-sense-voice/          # --model-dir 指向这里
    ├── model.int8.onnx          # SenseVoice int8 权重
    └── tokens.txt               # token 表
```

whisper-turbo 模式则需 `turbo-encoder.onnx` / `turbo-decoder.onnx` / `turbo-tokens.txt`。

`silero_vad.onnx`（VAD 切段用，~640KB）随 crate 提交（同 `crates/asr-server/silero_vad.onnx`），
Docker 镜像由 Dockerfile COPY 到 `/opt/asr-server/silero_vad.onnx`；裸跑时用
`--vad-model` 指向它。

### 从 streaming-speech 原部署迁移模型

原 GB10 部署模型在宿主 `~/asr-server-models/sherpa-sense-voice/`，迁移即把它移到
toolkit 的 `<workspace>/models/sherpa-sense-voice/`（或在 compose 里把宿主目录挂到
容器 `/models/sherpa-sense-voice`）。模型本身不变，无需重新下载。GB10 重新拉取见
Dockerfile 顶部注释（hf-mirror 命令）。

## 裸跑

```bash
# SenseVoice 多语种（默认），启用 from-source（白名单 = 抖音下载目录）
cargo run --release -p asr-server -- \
  --model-dir <workspace>/models/sherpa-sense-voice \
  --model sense-voice \
  --vad-model crates/asr-server/silero_vad.onnx \
  --bind 127.0.0.1:8091 \
  --source-allowlist <workspace>/downloads/douyin
```

启动参数完整列表见 `Args`（`src/main.rs`）：`--num-threads` / `--decode-timeout` /
`--max-source-bytes` / `--source-fetch-timeout` / `--language`（whisper） 等。

## 部署（G10）

容器化编排见 `deploy/asr-tts/`（compose）。Dockerfile（`crates/asr-server/Dockerfile`）
保留 streaming-speech 原样：standalone 构建（无父 workspace），故其 `custom-utils`
仍用 git rev 依赖；裸 workspace 构建则走 `[workspace.dependencies]` 的 path patch 版本。
容器内 bind `0.0.0.0:8091`，对外暴露由 compose `ports: 127.0.0.1:8091:8091` 限制为本机。

## 追踪

设环境变量 `TRACE_HUB_ENDPOINT` 即接入 trace-hub（与 toolkit-server 同源
`custom-utils` 0.15 `trace` feature）；未设则全程 no-op。入站 `traceparent` 透传，
`asr_transcribe` 顶层 span + `audio_decode` / `vad_segment` / `asr_decode` 子 span。
