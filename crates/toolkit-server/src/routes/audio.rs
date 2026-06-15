//! 音频（TTS）代理路由：toolkit-server 作为统一入口转发到上游 CosyVoice2
//! FastAPI 服务（参考 streaming-speech/server/tts 的接口）。
//!
//! 本阶段（Phase 1）只做「干净的代理 + 可观测」：
//!   - `POST /api/web/audio/tts`    → 上游 `POST /tts`，回传 WAV bytes
//!   - `GET  /api/web/audio/voices` → 上游 `GET /voices`，回传 JSON
//!
//! **不做**音频落盘 / 任务化（那是 Phase 3 AudioForge）。上游地址由环境变量
//! `TTS_BASE_URL` 配置（如 `http://192.168.0.68:8095`）；未配置时返回明确的 503。

use crate::state::AppState;
use axum::body::Bytes;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use custom_utils::trace::{self, SpanScope, SpanStatus, TraceContext};
use serde_json::{json, Value};
use std::time::Duration;

/// TTS 生成可能 10s+（CosyVoice2 首次请求要懒加载 ~5GB 进显存约 30s），给足超时。
const TTS_TIMEOUT: Duration = Duration::from_secs(180);
/// /voices 是轻量元数据查询，短超时即可。
const VOICES_TIMEOUT: Duration = Duration::from_secs(15);
/// 音频清洗（开 Demucs 的整段清洗）可能数分钟，与上游 `PROCESS_TIMEOUT_SEC` 对齐。
const CLEAN_TIMEOUT: Duration = Duration::from_secs(600);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tts", post(tts))
        .route("/voices", get(voices))
        .route("/clean", post(clean))
}

/// 读上游音频清洗 base URL。未配置 `CLEAN_BASE_URL` 返回 None → handler 回 503
/// （与 `TTS_BASE_URL` 的约定一致：env 缺失即 503，配置但不可达才 502）。
fn clean_base_url() -> Option<String> {
    std::env::var("CLEAN_BASE_URL")
        .ok()
        .map(|s| s.trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

fn clean_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "audio-cleanup upstream not configured",
            "hint": "set CLEAN_BASE_URL env (e.g. http://127.0.0.1:8097)",
        })),
    )
        .into_response()
}

/// `POST /api/web/audio/clean`：把入站 multipart 原样转发到上游 `POST /clean`，回传清洗后
/// 音频字节，并透传上游 `X-Cleanup-*` 元数据头。env 缺失 → 503；上游不可达 → 502。
async fn clean(headers: HeaderMap, body: Bytes) -> Response {
    let Some(base) = clean_base_url() else {
        return clean_unavailable();
    };
    let client = match reqwest::Client::builder().timeout(CLEAN_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => return bad_gateway(format!("build client: {e}")),
    };

    // 透传入站 Content-Type（含 multipart boundary）+ 原始 body。
    let mut req = client.post(format!("{base}/clean")).body(body.to_vec());
    if let Some(ct) = headers.get(header::CONTENT_TYPE) {
        req = req.header(header::CONTENT_TYPE, ct);
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return bad_gateway(format!("clean upstream request failed: {e}")),
    };
    let status = resp.status();
    let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    // 收集要透传的响应头：content-type + 所有 X-Cleanup-* 元数据。
    let mut out_headers = HeaderMap::new();
    for (name, value) in resp.headers() {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("content-type")
            || n.to_ascii_lowercase().starts_with("x-cleanup-")
        {
            if let (Ok(hn), Ok(hv)) = (
                HeaderName::from_bytes(name.as_ref()),
                HeaderValue::from_bytes(value.as_bytes()),
            ) {
                out_headers.insert(hn, hv);
            }
        }
    }

    match resp.bytes().await {
        Ok(bytes) => (code, out_headers, bytes).into_response(),
        Err(e) => bad_gateway(format!("read upstream body: {e}")),
    }
}

/// 读上游 TTS base URL。未配置返回 None → handler 回 503。
fn tts_base_url() -> Option<String> {
    std::env::var("TTS_BASE_URL")
        .ok()
        .map(|s| s.trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

fn service_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "TTS upstream not configured",
            "hint": "set TTS_BASE_URL env (e.g. http://192.168.0.68:8095)",
        })),
    )
        .into_response()
}

fn bad_gateway(msg: impl Into<String>) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({ "error": msg.into() })),
    )
        .into_response()
}

/// 从入站请求头提取 W3C `traceparent`，让 TTS 调用接入同一条 trace（有上游则
/// 作其子 span，无则起独立 trace）。trace 未启用时整段为 no-op。
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

/// `POST /api/web/audio/tts`：转发请求体到上游 `POST /tts`，回传 WAV bytes。
///
/// 请求体直接透传（`{text, voice_id, instruct?, prompt_text?, mode?}`），不做
/// schema 解析——上游负责校验。响应保持 `audio/wav`。
async fn tts(headers: HeaderMap, body: Bytes) -> Response {
    let Some(base) = tts_base_url() else {
        return service_unavailable();
    };
    let ctx = trace_root(&headers);

    // 两阶段 trace：anchor 先发，便于 trace-hub 立刻看到「TTS 请求进行中」（生成
    // 可能 10s+）。完成后 emit_end 覆盖填响应大小 / 状态。
    let scope = trace::enabled().then(|| {
        let summary = json!({
            "upstream": base,
            "text_len": serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(|s| s.chars().count())),
        });
        let scope = SpanScope::new(ctx, "tts_proxy")
            .with_flow_name("audio_tts")
            .with_summary(summary)
            .with_request_body(
                String::from_utf8_lossy(&body)
                    .chars()
                    .take(2048)
                    .collect::<String>(),
            );
        scope.emit_start();
        scope
    });

    let client = match reqwest::Client::builder().timeout(TTS_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            if let Some(s) = scope {
                s.emit_end(None, SpanStatus::Error(format!("client: {e}")), None);
            }
            return bad_gateway(format!("build client: {e}"));
        }
    };

    let url = format!("{base}/tts");
    let resp = client
        .post(&url)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.to_vec())
        .send()
        .await;

    match resp {
        Ok(r) => {
            let status = r.status();
            let ct = r
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("audio/wav")
                .to_string();
            match r.bytes().await {
                Ok(bytes) => {
                    if let Some(s) = scope {
                        let st = if status.is_success() {
                            SpanStatus::Ok
                        } else {
                            SpanStatus::Error(format!("upstream {status}"))
                        };
                        s.emit_end(
                            Some(format!("{} bytes ({ct})", bytes.len())),
                            st,
                            Some(json!({ "upstream_status": status.as_u16(), "resp_bytes": bytes.len() })),
                        );
                    }
                    // 透传上游状态码与 content-type（成功 → audio/wav；上游报错 → 原样回 JSON 错误体）。
                    let code =
                        StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
                    (code, [(header::CONTENT_TYPE, ct)], bytes).into_response()
                }
                Err(e) => {
                    if let Some(s) = scope {
                        s.emit_end(None, SpanStatus::Error(format!("read body: {e}")), None);
                    }
                    bad_gateway(format!("read upstream body: {e}"))
                }
            }
        }
        Err(e) => {
            if let Some(s) = scope {
                s.emit_end(None, SpanStatus::Error(format!("request: {e}")), None);
            }
            bad_gateway(format!("tts upstream request failed: {e}"))
        }
    }
}

/// `GET /api/web/audio/voices`：代理上游 `GET /voices`，回传音色库 JSON。
async fn voices(headers: HeaderMap) -> Response {
    let Some(base) = tts_base_url() else {
        return service_unavailable();
    };
    let ctx = trace_root(&headers);
    let scope = trace::enabled().then(|| {
        let scope = SpanScope::new(ctx, "tts_voices")
            .with_flow_name("audio_voices")
            .with_summary(json!({ "upstream": base }));
        scope.emit_start();
        scope
    });

    let client = match reqwest::Client::builder().timeout(VOICES_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            if let Some(s) = scope {
                s.emit_end(None, SpanStatus::Error(format!("client: {e}")), None);
            }
            return bad_gateway(format!("build client: {e}"));
        }
    };

    let url = format!("{base}/voices");
    match client.get(&url).send().await {
        Ok(r) => {
            let status = r.status();
            match r.text().await {
                Ok(text) => {
                    if let Some(s) = scope {
                        let st = if status.is_success() {
                            SpanStatus::Ok
                        } else {
                            SpanStatus::Error(format!("upstream {status}"))
                        };
                        s.emit_end(Some(text.chars().take(2048).collect()), st, None);
                    }
                    let code =
                        StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
                    let body: Value =
                        serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
                    (code, Json(body)).into_response()
                }
                Err(e) => {
                    if let Some(s) = scope {
                        s.emit_end(None, SpanStatus::Error(format!("read body: {e}")), None);
                    }
                    bad_gateway(format!("read upstream body: {e}"))
                }
            }
        }
        Err(e) => {
            if let Some(s) = scope {
                s.emit_end(None, SpanStatus::Error(format!("request: {e}")), None);
            }
            bad_gateway(format!("voices upstream request failed: {e}"))
        }
    }
}
