use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use clap::Parser;
use custom_utils::trace::{self, SpanRecord, SpanStatus, TraceContext};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
    OfflineWhisperModelConfig,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod vad;

#[derive(Parser, Debug, Clone)]
#[command(about = "Minimal OpenAI-compatible ASR HTTP service backed by sherpa-onnx")]
struct Args {
    /// 默认绑定 127.0.0.1（Plan C 安全默认值）。容器内仍需 0.0.0.0 才能被
    /// docker 端口转发到达——见 Dockerfile CMD / compose ports 注释。
    #[arg(long, default_value = "127.0.0.1:8091")]
    bind: String,

    #[arg(long)]
    model_dir: PathBuf,

    /// sense-voice | whisper-turbo
    #[arg(long, default_value = "sense-voice")]
    model: String,

    /// Whisper 解码语言，仅 whisper-turbo 生效；留空为自动检测
    #[arg(long, default_value = "")]
    language: String,

    /// 单个 recognizer 的 onnxruntime 线程数。默认按核数自适应
    /// (cores/2,夹在 [2,8]),老默认 2 在 GB10 20 核上浪费严重。
    #[arg(long, default_value_t = default_num_threads())]
    num_threads: i32,

    /// recognizer 池大小:>1 时并发请求不再整体串行(每个副本独占
    /// 一份模型内存,SenseVoice int8 约 ~250MB/份)。
    #[arg(long, default_value_t = 2)]
    pool_size: usize,

    #[arg(long, default_value_t = 50 * 1024 * 1024)]
    max_body_bytes: usize,

    /// silero_vad.onnx 路径（Plan B）。镜像内由 Dockerfile COPY 到该位置。
    #[arg(long, default_value = "/opt/asr-server/silero_vad.onnx")]
    vad_model: PathBuf,

    /// ffmpeg 解码超时（秒，Plan A）
    #[arg(long, default_value_t = 60)]
    decode_timeout: u64,

    /// from-source 端点白名单前缀（逗号分隔，Plan C）。为空则 from-source 禁用。
    #[arg(long, value_delimiter = ',')]
    source_allowlist: Vec<PathBuf>,

    /// from-source HTTP 下载体积上限（字节，Plan C）
    #[arg(long, default_value_t = 100 * 1024 * 1024)]
    max_source_bytes: u64,

    /// from-source HTTP 下载整体超时（秒，Plan C）
    #[arg(long, default_value_t = 30)]
    source_fetch_timeout: u64,
}

struct AppState {
    args: Args,
    is_whisper: bool,
    recognizers: RecognizerPool,
    /// canonical 化后的白名单前缀（Plan C）。空 = from-source 禁用。
    allowlist: Vec<PathBuf>,
}

fn default_num_threads() -> i32 {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    ((cores / 2).clamp(2, 8)) as i32
}

/// 固定大小的 recognizer 池。`acquire` 先 try_lock 轮询找空闲副本,
/// 全忙则阻塞在轮转到的槽位上(调用方都在 spawn_blocking 线程,可阻塞)。
struct RecognizerPool {
    items: Vec<Mutex<OfflineRecognizer>>,
    next: std::sync::atomic::AtomicUsize,
}

impl RecognizerPool {
    fn new(items: Vec<Mutex<OfflineRecognizer>>) -> Self {
        assert!(!items.is_empty());
        Self {
            items,
            next: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn acquire(&self) -> std::sync::MutexGuard<'_, OfflineRecognizer> {
        let n = self.items.len();
        let start = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        for i in 0..n {
            if let Ok(g) = self.items[(start + i) % n].try_lock() {
                return g;
            }
        }
        self.items[start % n]
            .lock()
            .expect("recognizer mutex poisoned")
    }
}

fn build_config(args: &Args) -> anyhow::Result<OfflineRecognizerConfig> {
    let p = |sub: &str| -> Option<String> {
        let path = args.model_dir.join(sub);
        path.exists().then(|| path.to_string_lossy().into_owned())
    };
    let mut config = OfflineRecognizerConfig::default();
    match args.model.as_str() {
        "sense-voice" => {
            config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
                model: p("model.int8.onnx"),
                language: Some("auto".into()),
                use_itn: true,
            };
            config.model_config.tokens = p("tokens.txt");
        }
        "whisper-turbo" => {
            let language = (!args.language.is_empty()).then(|| args.language.clone());
            config.model_config.whisper = OfflineWhisperModelConfig {
                encoder: p("turbo-encoder.onnx"),
                decoder: p("turbo-decoder.onnx"),
                language,
                task: Some("transcribe".into()),
                ..Default::default()
            };
            config.model_config.tokens = p("turbo-tokens.txt");
            config.model_config.model_type = Some("whisper".into());
        }
        other => anyhow::bail!(
            "unsupported --model: {other} (expected 'sense-voice' or 'whisper-turbo')"
        ),
    }
    config.model_config.num_threads = args.num_threads;
    Ok(config)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // 全链路追踪（trace-hub）：仅当设 TRACE_HUB_ENDPOINT 才启用，未设则全程 no-op。
    // 须在 tokio 运行时内 init（本函数是 #[tokio::main]，OK）。
    if let Ok(ep) = std::env::var("TRACE_HUB_ENDPOINT") {
        trace::init(trace::TraceConfig::new(ep, "asr-server"));
        tracing::info!("trace-hub tracing enabled");
    }

    let args = Args::parse();
    let is_whisper = args.model == "whisper-turbo";
    let pool_size = args.pool_size.max(1);
    tracing::info!(
        model = %args.model, dir = ?args.model_dir,
        pool_size, num_threads = args.num_threads,
        "loading recognizer pool"
    );
    let config = build_config(&args)?;
    let mut pool = Vec::with_capacity(pool_size);
    for i in 0..pool_size {
        let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| {
            anyhow::anyhow!("failed to create recognizer; check --model-dir and model files")
        })?;
        tracing::info!("recognizer {}/{} ready", i + 1, pool_size);
        pool.push(Mutex::new(recognizer));
    }

    // 白名单 canonical 化（Plan C）：避免 symlink / `..` 逃逸。无法 canonical 化的
    // 条目（如目录不存在）丢弃并 warn。
    let allowlist: Vec<PathBuf> = args
        .source_allowlist
        .iter()
        .filter_map(|p| match p.canonicalize() {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!(
                    "--source-allowlist entry {:?} dropped (canonicalize failed: {e})",
                    p
                );
                None
            }
        })
        .collect();
    if allowlist.is_empty() {
        tracing::warn!(
            "--source-allowlist empty: /v1/audio/transcriptions/from-source is DISABLED"
        );
    } else {
        tracing::info!(?allowlist, "from-source enabled with allowlist");
    }
    if !args.vad_model.exists() {
        tracing::warn!(
            "vad model not found at {:?}; vad=true requests will fail",
            args.vad_model
        );
    }

    let max_body = args.max_body_bytes;
    let bind = args.bind.clone();
    let state = Arc::new(AppState {
        args,
        is_whisper,
        recognizers: RecognizerPool::new(pool),
        allowlist,
    });

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/models", get(list_models))
        .route("/v1/audio/transcriptions", post(transcribe))
        .route("/v1/audio/transcriptions/from-source", post(from_source))
        .layer(DefaultBodyLimit::max(max_body))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("listening on http://{}", bind);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn healthz() -> &'static str {
    "ok"
}

#[derive(Serialize)]
struct ModelEntry {
    id: String,
    object: &'static str,
}

#[derive(Serialize)]
struct ModelList {
    object: &'static str,
    data: Vec<ModelEntry>,
}

async fn list_models(State(state): State<Arc<AppState>>) -> Json<ModelList> {
    Json(ModelList {
        object: "list",
        data: vec![ModelEntry {
            id: state.args.model.clone(),
            object: "model",
        }],
    })
}

#[derive(Serialize)]
struct TranscriptionResponse {
    text: String,
    /// 仅 vad=true 时存在（Plan B）；不传 vad 时省略，保持向后兼容。
    #[serde(skip_serializing_if = "Option::is_none")]
    segments: Option<Vec<SegmentOut>>,
}

#[derive(Serialize)]
struct SegmentOut {
    start: f64,
    end: f64,
    text: String,
}

#[derive(Serialize, Debug)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Serialize, Debug)]
struct ErrorBody {
    message: String,
    r#type: &'static str,
}

type ApiError = (StatusCode, Json<ErrorResponse>);

fn err(status: StatusCode, kind: &'static str, msg: impl Into<String>) -> ApiError {
    (
        status,
        Json(ErrorResponse {
            error: ErrorBody {
                message: msg.into(),
                r#type: kind,
            },
        }),
    )
}

fn parse_bool_field(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1")
}

// ===================== 全链路追踪辅助（trace-hub）=====================

/// 从入站请求头提取 traceparent：有上游则 `.child()` 出本服务子树根（父=上游 span），
/// 无则起独立 trace。该 ctx 即本次请求 `asr_transcribe` span 的身份；其子 span 用 `ctx.child()`。
fn trace_root(headers: &HeaderMap) -> TraceContext {
    trace::extract_traceparent(|h| {
        headers
            .get(h)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    })
    .map(|remote| remote.child())
    .unwrap_or_else(TraceContext::root)
}

/// 以 `span` 自身为身份记一个 span（顶层 span 传 ctx；子 span 传 `ctx.child()`）。
/// 调用方须先 `trace::enabled()` 判断后再构造 payload，避免关闭时白做 JSON/clone。
fn emit_span(
    span: &TraceContext,
    kind: &str,
    start_ms: i64,
    end_ms: i64,
    summary: serde_json::Value,
    detail: serde_json::Value,
    response_body: Option<String>,
) {
    trace::record_span(SpanRecord {
        trace_id: span.trace_id.clone(),
        span_id: span.span_id.clone(),
        parent_span_id: span.parent_span_id.clone(),
        service: String::new(), // record_span 自动填 "asr-server"
        kind: kind.to_string(),
        flow_name: None,
        start_ms,
        end_ms,
        status: SpanStatus::Ok,
        summary,
        detail,
        request_body: None,
        response_body,
        body_truncated: false,
        links: Vec::new(),
    });
}

/// 解码 + 识别的统一入口，集中记 `asr_transcribe`（顶层）与 `audio_decode`（子）span，
/// `vad_segment` / `asr_decode` 子 span 在阻塞线程内记（见 `transcribe_blocking`）。
/// 两个 handler 共用此函数，避免重复埋点。
async fn run_traced(
    state: Arc<AppState>,
    ctx: TraceContext,
    _t0: i64,
    bytes: Vec<u8>,
    vad_flag: bool,
    source_kind: &'static str,
) -> Result<TranscriptionResponse, ApiError> {
    let model = state.args.model.clone();

    // asr_transcribe 顶层 span：两阶段 emit。anchor 在解码/识别之前发——trace-hub
    // 立刻能看到 ASR 请求在进行，输入 bytes 大小已可见；长音频（>30s ffmpeg + 多段
    // VAD + 逐段推理）期间 UI 不再空着。完成后 emit_end 覆盖填全文。
    let transcribe_scope = trace::enabled().then(|| {
        let scope = trace::SpanScope::new(ctx.clone(), "asr_transcribe")
            .with_summary(serde_json::json!({
                "model": model,
                "vad": vad_flag,
                "source_kind": source_kind,
                "input_bytes": bytes.len(),
            }))
            .with_request_body(format!("source={} bytes={}", source_kind, bytes.len()));
        scope.emit_start();
        scope
    });

    // audio_decode 子 span（短，一阶段记完即可）
    let d0 = trace::now_ms();
    let fast = is_fast_wav(&bytes);
    let samples = decode_any(&bytes, &state.args).await?;
    let d1 = trace::now_ms();
    let n_samples = samples.len();
    if trace::enabled() {
        emit_span(
            &ctx.child(),
            "audio_decode",
            d0,
            d1,
            serde_json::json!({ "fast_path": fast, "samples": n_samples, "decode_ms": (d1 - d0).max(0) }),
            serde_json::Value::Null,
            None,
        );
    }

    // 识别（vad_segment + 逐段 asr_decode 在阻塞线程内记）
    let resp = run_transcription(state, samples, vad_flag, ctx.clone()).await?;

    if let Some(scope) = transcribe_scope {
        let segments_count = resp.segments.as_ref().map(|s| s.len()).unwrap_or(0);
        scope.emit_end(
            Some(resp.text.clone()), // 转写全文 = ASR 的「body」
            SpanStatus::Ok,
            Some(serde_json::json!({
                "text_len": resp.text.chars().count(),
                "segments_count": segments_count,
            })),
        );
    }
    Ok(resp)
}

// ===================== /v1/audio/transcriptions (multipart) =====================

async fn transcribe(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<TranscriptionResponse>, ApiError> {
    let ctx = trace_root(&headers);
    let t0 = trace::now_ms();
    let mut audio_bytes: Option<Vec<u8>> = None;
    let mut vad_flag = false;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            format!("multipart: {e}"),
        )
    })? {
        match field.name().unwrap_or("") {
            "file" => {
                let bytes = field.bytes().await.map_err(|e| {
                    err(
                        StatusCode::BAD_REQUEST,
                        "invalid_request",
                        format!("read file: {e}"),
                    )
                })?;
                audio_bytes = Some(bytes.to_vec());
            }
            "vad" => {
                let v = field.text().await.map_err(|e| {
                    err(
                        StatusCode::BAD_REQUEST,
                        "invalid_request",
                        format!("read vad: {e}"),
                    )
                })?;
                vad_flag = parse_bool_field(&v);
            }
            // 其它字段（model/language/response_format/prompt）忽略，
            // 服务端通过启动参数固定模型。
            _ => {}
        }
    }

    let bytes = audio_bytes.ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing field 'file'",
        )
    })?;
    let resp = run_traced(state, ctx, t0, bytes, vad_flag, "file").await?;
    Ok(Json(resp))
}

// ===================== /v1/audio/transcriptions/from-source (JSON) =====================

#[derive(Deserialize)]
struct FromSourceRequest {
    source: String,
    #[serde(default)]
    vad: bool,
}

async fn from_source(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<FromSourceRequest>,
) -> Result<Json<TranscriptionResponse>, ApiError> {
    let ctx = trace_root(&headers);
    let t0 = trace::now_ms();
    if state.allowlist.is_empty() {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "endpoint_disabled",
            "from-source disabled; configure --source-allowlist",
        ));
    }

    // _tmp 持有 HTTP 下载临时文件的 Drop guard：函数返回（成功或出错）即删文件。
    let _tmp;
    let bytes: Vec<u8>;

    if let Some(rest) = req.source.strip_prefix("file://") {
        let path = validate_file_path(&req.source, rest)?;
        if !path.exists() {
            return Err(err(
                StatusCode::NOT_FOUND,
                "not_found",
                "source file not found",
            ));
        }
        let canon = path
            .canonicalize()
            .map_err(|_| err(StatusCode::NOT_FOUND, "not_found", "source file not found"))?;
        if !path_in_allowlist(&canon, &state.allowlist) {
            return Err(err(
                StatusCode::FORBIDDEN,
                "forbidden_source",
                "path not in --source-allowlist",
            ));
        }
        bytes = tokio::fs::read(&canon).await.map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                format!("read source: {e}"),
            )
        })?;
    } else if req.source.starts_with("http://") || req.source.starts_with("https://") {
        let (b, guard) = fetch_http(
            &req.source,
            state.args.max_source_bytes,
            state.args.source_fetch_timeout,
        )
        .await?;
        bytes = b;
        _tmp = guard;
    } else {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "unsupported source scheme",
        ));
    }

    let source_kind = if req.source.starts_with("http") {
        "http"
    } else {
        "file"
    };
    let resp = run_traced(state, ctx, t0, bytes, req.vad, source_kind).await?;
    Ok(Json(resp))
}

/// `file://` 路径合法性校验（纯函数，不碰文件系统）。`rest` 是去掉 `file://`
/// 前缀后的部分，对 `file:///abs/path` 即 `/abs/path`。
fn validate_file_path(source: &str, rest: &str) -> Result<PathBuf, ApiError> {
    // 拒绝 URL 编码字符：避免 %2e%2e 之类绕过 canonical、以及多重解码歧义。
    if source.contains('%') {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "encoded file:// path not supported",
        ));
    }
    // file:///abs → rest 以 '/' 开头；否则形如 file://host/... 或缺斜杠，不支持。
    if !rest.starts_with('/') {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "unsupported file:// path",
        ));
    }
    // 拒绝 Windows 风格 file:///C:/...（本服务只跑 GB10 linux）。
    if is_windows_style(rest) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "unsupported file:// path",
        ));
    }
    Ok(PathBuf::from(rest))
}

/// `/C:/...` 形态判定（去掉 file:// 后）。
fn is_windows_style(rest: &str) -> bool {
    let b = rest.as_bytes();
    b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':'
}

fn path_in_allowlist(canon: &Path, allow: &[PathBuf]) -> bool {
    allow.iter().any(|prefix| canon.starts_with(prefix))
}

/// 处理完即删的临时文件 guard。
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 流式下载 HTTP(S) 到临时文件，边读边累计字节数，超 `max_bytes` 立刻中止；
/// 整次受 `timeout_secs` 约束。返回文件内容 + Drop guard（调用方持有以延后删除）。
async fn fetch_http(
    url: &str,
    max_bytes: u64,
    timeout_secs: u64,
) -> Result<(Vec<u8>, TempFile), ApiError> {
    let dir = PathBuf::from("/tmp/asr-input");
    tokio::fs::create_dir_all(&dir).await.ok();
    let tmp_path = dir.join(format!("{}.bin", uuid::Uuid::new_v4()));
    let guard = TempFile(tmp_path.clone());

    let fetch = async {
        let resp = reqwest::get(url).await.map_err(|e| {
            err(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                format!("fetch failed: {e}"),
            )
        })?;
        let mut file = tokio::fs::File::create(&tmp_path).await.map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                format!("temp create: {e}"),
            )
        })?;
        let mut stream = resp.bytes_stream();
        let mut total: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                err(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    format!("fetch read: {e}"),
                )
            })?;
            total += chunk.len() as u64;
            if total > max_bytes {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "fetch too large",
                ));
            }
            file.write_all(&chunk).await.map_err(|e| {
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    format!("temp write: {e}"),
                )
            })?;
        }
        file.flush().await.ok();
        Ok::<(), ApiError>(())
    };

    match tokio::time::timeout(Duration::from_secs(timeout_secs), fetch).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e), // guard 在此 drop → 临时文件删除
        Err(_) => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "fetch timeout",
            ))
        }
    }

    let bytes = tokio::fs::read(&tmp_path).await.map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            format!("read temp: {e}"),
        )
    })?;
    Ok((bytes, guard))
}

// ===================== 解码：任意输入 → 16k mono f32 PCM (Plan A) =====================

async fn decode_any(bytes: &[u8], args: &Args) -> Result<Vec<f32>, ApiError> {
    let samples = if is_fast_wav(bytes) {
        decode_wav_16k_mono(bytes)
            .map_err(|e| err(StatusCode::BAD_REQUEST, "invalid_request", e))?
    } else {
        ffmpeg_decode(bytes, args.decode_timeout).await?
    };
    // 解码出的音频 < 0.1 s（1600 样本 @16k）视为无效输入。
    if samples.len() < 1600 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "decoded audio too short",
        ));
    }
    Ok(samples)
}

/// 只读 WAV 头判定能否走快路径：RIFF/WAVE + fmt chunk 满足
/// channels==1 && sample_rate==16000 && bits∈{16,32} && audio_format∈{1=PCM,3=Float}。
/// 任何不满足 / 解析失败 → false（交给 ffmpeg 慢路径）。
fn is_fast_wav(b: &[u8]) -> bool {
    if b.len() < 44 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return false;
    }
    let mut off = 12usize;
    while off + 8 <= b.len() {
        let cid = &b[off..off + 4];
        let csz = u32::from_le_bytes([b[off + 4], b[off + 5], b[off + 6], b[off + 7]]) as usize;
        let body = off + 8;
        if cid == b"fmt " {
            if body + 16 > b.len() {
                return false;
            }
            let audio_format = u16::from_le_bytes([b[body], b[body + 1]]);
            let channels = u16::from_le_bytes([b[body + 2], b[body + 3]]);
            let sample_rate =
                u32::from_le_bytes([b[body + 4], b[body + 5], b[body + 6], b[body + 7]]);
            let bits = u16::from_le_bytes([b[body + 14], b[body + 15]]);
            return channels == 1
                && sample_rate == 16000
                && (bits == 16 || bits == 32)
                && (audio_format == 1 || audio_format == 3);
        }
        // chunk 按偶数字节对齐。
        off = body + csz + (csz & 1);
    }
    false
}

/// 走 ffmpeg：stdin 喂原始 bytes，stdout 拿 16k mono s16le PCM，全程不落盘。
async fn ffmpeg_decode(bytes: &[u8], timeout_secs: u64) -> Result<Vec<f32>, ApiError> {
    use tokio::process::Command;

    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            "pipe:0",
            "-f",
            "s16le",
            "-ar",
            "16000",
            "-ac",
            "1",
            "-acodec",
            "pcm_s16le",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                format!("spawn ffmpeg: {e}"),
            )
        })?;

    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");

    // 同时写 stdin / 读 stdout / 读 stderr，避免任一管道写满造成死锁。
    let input = bytes.to_vec();
    let write_task = tokio::spawn(async move {
        let _ = stdin.write_all(&input).await;
        // stdin 在此 drop → 向 ffmpeg 发 EOF
    });
    let out_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf).await;
        buf
    });
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });

    let status = match tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait()).await {
        Ok(s) => s.map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                format!("ffmpeg wait: {e}"),
            )
        })?,
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(err(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "decode timeout",
            ));
        }
    };

    let _ = write_task.await;
    let stdout_buf = out_task.await.unwrap_or_default();
    let stderr_buf = err_task.await.unwrap_or_default();

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_buf);
        let snippet: String = stderr.chars().take(200).collect();
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            format!("ffmpeg decode failed: {snippet}"),
        ));
    }

    let samples: Vec<f32> = stdout_buf
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();
    Ok(samples)
}

// ===================== 识别（含 VAD 切段，Plan B） =====================

async fn run_transcription(
    state: Arc<AppState>,
    samples: Vec<f32>,
    vad_flag: bool,
    ctx: TraceContext,
) -> Result<TranscriptionResponse, ApiError> {
    if vad_flag && !state.args.vad_model.exists() {
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            format!("vad model not available: {:?}", state.args.vad_model),
        ));
    }
    tokio::task::spawn_blocking(move || transcribe_blocking(&state, samples, vad_flag, &ctx))
        .await
        .map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                format!("join: {e}"),
            )
        })?
        .map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                e.to_string(),
            )
        })
}

/// 单段识别 + 记一条 `asr_decode` 子 span（trace 关闭时仅多一次 now_ms，无 JSON/clone）。
fn recognize_traced(
    recognizer: &OfflineRecognizer,
    samples: &[f32],
    is_whisper: bool,
    long: bool,
    ctx: &TraceContext,
    seg_index: usize,
    seg_dur_s: f64,
) -> anyhow::Result<String> {
    let a0 = trace::now_ms();
    let text = if long {
        recognize_long(recognizer, samples, is_whisper)?
    } else {
        recognize(recognizer, samples)?
    };
    if trace::enabled() {
        let a1 = trace::now_ms();
        emit_span(
            &ctx.child(),
            "asr_decode",
            a0,
            a1,
            serde_json::json!({
                "seg_index": seg_index,
                "seg_dur_s": seg_dur_s,
                "decode_ms": (a1 - a0).max(0),
                "text_len": text.chars().count(),
            }),
            serde_json::Value::Null,
            if text.is_empty() {
                None
            } else {
                Some(text.clone())
            }, // 该段文本 = body
        );
    }
    Ok(text)
}

/// 阻塞执行：持 recognizer 锁，按 vad_flag 走单段 / 多段。`ctx` 为本请求 span，
/// 子 span（vad_segment / asr_decode）记为其 child。record_span 内部用 try_send，
/// 在 spawn_blocking 线程上安全（不依赖 tokio 运行时上下文）。
fn transcribe_blocking(
    state: &AppState,
    samples: Vec<f32>,
    vad_flag: bool,
    ctx: &TraceContext,
) -> anyhow::Result<TranscriptionResponse> {
    let recognizer = state.recognizers.acquire();
    let total_dur = samples.len() as f64 / vad::SAMPLE_RATE as f64;

    if !vad_flag {
        let text = recognize_traced(
            &recognizer,
            &samples,
            state.is_whisper,
            false,
            ctx,
            0,
            total_dur,
        )?;
        return Ok(TranscriptionResponse {
            text,
            segments: None,
        });
    }

    // 音频 < 1 s：跳过 VAD，整段直推，segments 退化为单元素。
    if samples.len() < vad::SAMPLE_RATE as usize {
        let text = recognize_traced(
            &recognizer,
            &samples,
            state.is_whisper,
            false,
            ctx,
            0,
            total_dur,
        )?;
        return Ok(TranscriptionResponse {
            text: text.clone(),
            segments: Some(vec![SegmentOut {
                start: 0.0,
                end: total_dur,
                text,
            }]),
        });
    }

    let vad_model = state
        .args
        .vad_model
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("vad model path not utf-8"))?;
    let v0 = trace::now_ms();
    let segs = vad::segment(vad_model, &samples)?;
    if trace::enabled() {
        let v1 = trace::now_ms();
        emit_span(
            &ctx.child(),
            "vad_segment",
            v0,
            v1,
            serde_json::json!({ "segments_count": segs.len(), "vad_ms": (v1 - v0).max(0) }),
            serde_json::json!({
                "boundaries": segs.iter().map(|s| serde_json::json!({ "start": s.start, "end": s.end })).collect::<Vec<_>>()
            }),
            None,
        );
    }

    let mut segments = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        let text = recognize_traced(
            &recognizer,
            &s.samples,
            state.is_whisper,
            true,
            ctx,
            i,
            s.end - s.start,
        )?;
        if text.is_empty() {
            continue;
        }
        segments.push(SegmentOut {
            start: s.start,
            end: s.end,
            text,
        });
    }
    let text = segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    Ok(TranscriptionResponse {
        text,
        segments: Some(segments),
    })
}

fn recognize(recognizer: &OfflineRecognizer, samples: &[f32]) -> anyhow::Result<String> {
    let stream = recognizer.create_stream();
    stream.accept_waveform(vad::SAMPLE_RATE, samples);
    recognizer.decode(&stream);
    let result = stream
        .get_result()
        .ok_or_else(|| anyhow::anyhow!("recognizer returned no result"))?;
    Ok(result.text.trim().to_string())
}

/// 段内超 30 s 且 whisper 模式时按 25 s 窗 + 2 s 重叠（步长 23 s）硬切，
/// 逐窗推理后拼接（重叠区交给后一窗，前窗边界更易吞字）。其它情况整段直推。
fn recognize_long(
    recognizer: &OfflineRecognizer,
    samples: &[f32],
    is_whisper: bool,
) -> anyhow::Result<String> {
    const SR: usize = vad::SAMPLE_RATE as usize;
    if !is_whisper || samples.len() <= 30 * SR {
        return recognize(recognizer, samples);
    }
    let win = 25 * SR;
    let step = 23 * SR;
    let mut texts = Vec::new();
    let mut i = 0;
    while i < samples.len() {
        let end = (i + win).min(samples.len());
        let t = recognize(recognizer, &samples[i..end])?;
        if !t.is_empty() {
            texts.push(t);
        }
        if end == samples.len() {
            break;
        }
        i += step;
    }
    Ok(texts.join(" "))
}

fn decode_wav_16k_mono(bytes: &[u8]) -> Result<Vec<f32>, String> {
    let cursor = std::io::Cursor::new(bytes);
    let reader = hound::WavReader::new(cursor).map_err(|e| format!("not a WAV file: {e}"))?;
    let spec = reader.spec();
    if spec.sample_rate != 16000 {
        return Err(format!(
            "sample_rate must be 16000, got {}",
            spec.sample_rate
        ));
    }
    if spec.channels != 1 {
        return Err(format!("channels must be 1 (mono), got {}", spec.channels));
    }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let denom = (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 / denom))
                .collect::<Result<_, _>>()
                .map_err(|e| format!("decode pcm: {e}"))?
        }
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| format!("decode float: {e}"))?,
    };
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_header(channels: u16, sample_rate: u32, bits: u16, audio_format: u16) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&audio_format.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&sample_rate.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes()); // byte rate
        v.extend_from_slice(&0u16.to_le_bytes()); // block align
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&0u32.to_le_bytes());
        v
    }

    #[test]
    fn fast_wav_accepts_16k_mono_pcm16() {
        assert!(is_fast_wav(&wav_header(1, 16000, 16, 1)));
    }

    #[test]
    fn fast_wav_accepts_16k_mono_float32() {
        assert!(is_fast_wav(&wav_header(1, 16000, 32, 3)));
    }

    #[test]
    fn fast_wav_rejects_stereo_and_44k() {
        assert!(!is_fast_wav(&wav_header(2, 16000, 16, 1)));
        assert!(!is_fast_wav(&wav_header(1, 44100, 16, 1)));
        assert!(!is_fast_wav(&wav_header(1, 16000, 24, 1))); // 24-bit → ffmpeg
    }

    #[test]
    fn fast_wav_rejects_non_riff() {
        assert!(!is_fast_wav(b"not an audio file at all........"));
        assert!(!is_fast_wav(&[]));
    }

    #[test]
    fn bool_field_parsing() {
        assert!(parse_bool_field("true"));
        assert!(parse_bool_field("1"));
        assert!(parse_bool_field(" TRUE "));
        assert!(!parse_bool_field("false"));
        assert!(!parse_bool_field("0"));
        assert!(!parse_bool_field(""));
    }

    #[test]
    fn file_path_rejects_percent_encoding() {
        let r = validate_file_path("file:///tmp/%2e%2e/etc/passwd", "/tmp/%2e%2e/etc/passwd");
        let (code, _) = r.unwrap_err();
        assert_eq!(code, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn file_path_rejects_windows_style() {
        assert!(is_windows_style("/C:/Users/x"));
        let r = validate_file_path("file:///C:/Users/x", "/C:/Users/x");
        assert!(r.is_err());
    }

    #[test]
    fn file_path_accepts_posix_abs() {
        let p = validate_file_path("file:///tmp/a.mp4", "/tmp/a.mp4").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/a.mp4"));
    }

    #[test]
    fn allowlist_prefix_check() {
        let allow = vec![PathBuf::from("/home/fengqi/.config/zero/downloads")];
        assert!(path_in_allowlist(
            Path::new("/home/fengqi/.config/zero/downloads/douyin/x.mp4"),
            &allow
        ));
        assert!(!path_in_allowlist(Path::new("/etc/passwd"), &allow));
    }
}
