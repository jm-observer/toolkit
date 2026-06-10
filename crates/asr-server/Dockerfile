# asr-server (arm64 / GB10) — CPU-only v1
#
# v1 范围:CPU SenseVoice int8。GPU/CUDA13 是后续工作(需手 build sherpa-onnx
# native libs for arm64+CUDA13,prebuilt 只到 CUDA 12.x;memory project_gpu_build.md
# 记录了 Windows 端 GPU 设置经验,Linux arm64 类似但 toolchain 不同)。
#
# 模型(宿主):放到 <workspace>/models/sherpa-sense-voice/(由 compose 挂到容器
# /models/sherpa-sense-voice),需含 model.int8.onnx 和 tokens.txt。从 streaming-speech
# 原部署迁移即把 ~/asr-server-models/sherpa-sense-voice/ 移到该路径(模型不变)。
# GB10 首次从 hf-mirror 拉(实测可用,GitHub 直连超时):
#   cd ~/asr-server-models/sherpa-sense-voice && \
#   curl -L -O https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/model.int8.onnx && \
#   curl -L -O https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/tokens.txt

FROM rust:1-bookworm AS build

# Crates 国内镜像(GB10 网络)。改用 rsproxy.cn:USTC 镜像同步较慢,新发布的
# custom-utils 0.15.0 当时只有 rsproxy 有(USTC 仍停在 0.14.3)。保留
# multiplexing=false 关 HTTP/2 抗 rsproxy 抖动(<10B/s 超时 / framing 错误)。
# 备用源(USTC,同步落后时回切):sparse+https://mirrors.ustc.edu.cn/crates.io-index/
RUN mkdir -p $CARGO_HOME && printf '%s\n' \
    '[source.crates-io]' \
    "replace-with = 'mirror'" \
    '[source.mirror]' \
    'registry = "sparse+https://rsproxy.cn/index/"' \
    '[net]' \
    'retry = 10' \
    '[http]' \
    'timeout = 120' \
    'multiplexing = false' \
    > $CARGO_HOME/config.toml

# reqwest 0.13 的 rustls 后端拉 aws-lc-rs，其 aws-lc-sys 构建期需 cmake（0.12 的 ring 不需要）。
RUN apt-get update && apt-get install -y --no-install-recommends cmake && rm -rf /var/lib/apt/lists/*

WORKDIR /src
# 本 crate 迁入 toolkit workspace 后，crates/asr-server/Cargo.toml 用的是
# `version.workspace = true` / `custom-utils = { workspace = true }`（path patch
# 指向 ../custom-utils）。这些在「无父 workspace」的独立 Docker 构建里不可用。
# 故镜像构建时就地生成一份 standalone Cargo.toml（git-rev 拉 custom-utils，与
# streaming-speech 原始 Dockerfile 同策略），crate 源码本身保持 workspace-native。
COPY src ./src
COPY silero_vad.onnx ./silero_vad.onnx
RUN printf '%s\n' \
    '[package]' \
    'name = "asr-server"' \
    'version = "0.1.0"' \
    'edition = "2021"' \
    '[dependencies]' \
    'axum = { version = "0.8", features = ["multipart"] }' \
    'tokio = { version = "1", features = ["full"] }' \
    'sherpa-onnx = { version = "1.13.0", default-features = false, features = ["shared"] }' \
    'hound = "3.5"' \
    'serde = { version = "1", features = ["derive"] }' \
    'serde_json = "1"' \
    'clap = { version = "4", features = ["derive"] }' \
    'tracing = "0.1"' \
    'tracing-subscriber = { version = "0.3", features = ["env-filter"] }' \
    'anyhow = "1"' \
    'reqwest = { version = "0.13", default-features = false, features = ["stream", "rustls", "http2"] }' \
    'futures-util = "0.3"' \
    'uuid = { version = "1", features = ["v4"] }' \
    'custom-utils = { git = "https://github.com/jm-observer/custom-utils.git", rev = "6254e583ba356b08918c183d465737b3b4652caa", default-features = false, features = ["trace"] }' \
    > Cargo.toml
# sherpa-onnx 用 features=["shared"]:sherpa-onnx-sys/build.rs 会从其 release 下
# linux-aarch64 prebuilt(github)。若 GB10 拉不到,可在宿主预下载并通过
# `--build-arg SHERPA_PREBUILT_URL=file://...` 或挂载方式注入,然后改用
# SHERPA_ONNX_LIB_DIR 跳过 build.rs 的下载。当前先尝试 build.rs 默认行为。
#
# registry 缓存挂载:镜像源抽风时,重试能复用已下载的 crate,不必每次从头拉。
# 注意只缓存 registry——target 不能缓存,否则下方 COPY --from=build 取不到产物。
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --bin asr-server

FROM debian:bookworm-slim
# ffmpeg(Plan A 多格式输入)来自 debian apt,不走 GitHub。约 +60MB,可接受。
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libgomp1 ffmpeg && rm -rf /var/lib/apt/lists/*

# silero_vad.onnx(Plan B):COPY 进镜像而非挂载——避免再造外部契约,
# 挂载只留给 sherpa 那种 >100MB 的生产模型。
COPY --from=build /src/silero_vad.onnx /opt/asr-server/silero_vad.onnx

# sherpa-onnx-sys 把 prebuilt arm64 libs 解到 /src/target/release/ 根目录(实测路径)。
# 三个运行期必备:libsherpa-onnx-c-api.so / libonnxruntime.so / libsherpa-onnx-cxx-api.so。
COPY --from=build /src/target/release/asr-server               /usr/local/bin/asr-server
COPY --from=build /src/target/release/libsherpa-onnx-c-api.so  /usr/local/lib/sherpa/
COPY --from=build /src/target/release/libonnxruntime.so        /usr/local/lib/sherpa/
COPY --from=build /src/target/release/libsherpa-onnx-cxx-api.so /usr/local/lib/sherpa/
ENV LD_LIBRARY_PATH=/usr/local/lib/sherpa

EXPOSE 8091
ENTRYPOINT ["/usr/local/bin/asr-server"]
# 容器内必须 bind 0.0.0.0 才能被 docker 端口转发到达(转发连的是容器 eth0,
# 不是 loopback)。对外的 localhost 限制由 compose `ports: 127.0.0.1:8091:8091`
# 完成。--source-allowlist 不在此设默认值(部署相关),由 compose command 注入。
CMD ["--bind", "0.0.0.0:8091", "--model", "sense-voice", "--model-dir", "/models/sherpa-sense-voice", "--vad-model", "/opt/asr-server/silero_vad.onnx"]
