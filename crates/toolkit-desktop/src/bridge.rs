//! 本地 HTTP 桥 — login 远端窗口 ↔ desktop 后端 + desktop ↔ G10 web 上下文交换。
//!
//! 设计：login 窗加载 `https://www.douyin.com`（远端 origin），Tauri 2 默认不让远端
//! 页面调任何 `#[tauri::command]`；同样，G10 web UI 跑在另一 origin（如 http://g10:8788）
//! 想读 desktop 本机状态也是跨 origin。这里统一在 `127.0.0.1:BRIDGE_PORT` 起 axum，
//! CORS 全开（私网仅本机可达，远端摸不到），承担：
//!
//! - **接收（写）**：login_hook.js fetch 喂 msToken / 通用 signal
//! - **暴露（读）**：G10 web UI fetch 拿 desktop 实时上下文（当前 URL、cookie、msToken、ths）
//!
//! 路由：
//! - `GET  /health`           — 探活
//! - `GET  /mstoken?value=`   — login_hook 上传 msToken
//! - `GET  /signal?name=&value=` — 通用 JS→Rust 槽
//! - `GET  /context`          — 完整 desktop 上下文，G10 web 周期 poll
//! - `GET  /login-url`        — 仅当前 login 窗 URL（轻量）

use crate::ths;
use crate::uploader::UploaderState;
use axum::extract::{Query, State};
use axum::response::Json;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tower_http::cors::{Any, CorsLayer};

pub const BRIDGE_PORT: u16 = 28788;

#[derive(Clone)]
pub struct BridgeCtx {
    pub app: AppHandle,
    pub uploader: Arc<UploaderState>,
    pub workspace: PathBuf,
}

#[derive(Debug, Deserialize)]
struct TokenQuery {
    value: String,
}

#[derive(Debug, Deserialize)]
struct SignalQuery {
    name: String,
    #[serde(default)]
    value: String,
}

async fn capture_mstoken(
    State(ctx): State<BridgeCtx>,
    Query(q): Query<TokenQuery>,
) -> &'static str {
    record_ms_token(&ctx.uploader, q.value).await
}

async fn signal(State(ctx): State<BridgeCtx>, Query(q): Query<SignalQuery>) -> &'static str {
    match q.name.as_str() {
        "mstoken" => record_ms_token(&ctx.uploader, q.value).await,
        other => {
            log::debug!("bridge signal unhandled name={other} len={}", q.value.len());
            "unknown_signal"
        }
    }
}

async fn record_ms_token(state: &UploaderState, value: String) -> &'static str {
    let trimmed = value.trim().to_string();
    if trimmed.len() < 16 {
        return "skip";
    }
    let mut slot = state.ms_token.lock().await;
    let changed = slot.as_deref() != Some(trimmed.as_str());
    if changed {
        log::info!("bridge captured new msToken (len={})", trimmed.len());
        *slot = Some(trimmed);
        *state.last_hash.lock().await = None;
    }
    "ok"
}

async fn health() -> &'static str {
    "bridge ok"
}

async fn login_url(State(ctx): State<BridgeCtx>) -> Json<Value> {
    let (has_window, url) = match ctx.app.get_webview_window("login") {
        Some(w) => (true, w.url().ok().map(|u| u.to_string())),
        None => (false, None),
    };
    Json(json!({ "has_window": has_window, "url": url }))
}

async fn context(State(ctx): State<BridgeCtx>) -> Json<Value> {
    // login 窗当前 URL + cookie 概览
    let (login_has_window, login_url, login_cookies_count) = match ctx.app.get_webview_window("login") {
        Some(w) => {
            let u = w.url().ok().map(|u| u.to_string());
            let count = "https://www.douyin.com"
                .parse::<url::Url>()
                .ok()
                .and_then(|target| w.cookies_for_url(target).ok())
                .map(|cs| cs.len() as i64);
            (true, u, count)
        }
        None => (false, None, None),
    };

    let ms_token_present = ctx.uploader.ms_token.lock().await.is_some();
    let ms_token_len = ctx
        .uploader
        .ms_token
        .lock()
        .await
        .as_ref()
        .map(|s| s.len() as i64);

    let ths_report = ths::status_report(&ctx.workspace);

    Json(json!({
        "login": {
            "has_window": login_has_window,
            "url": login_url,
            "cookies_count": login_cookies_count,
        },
        "ms_token": {
            "present": ms_token_present,
            "length": ms_token_len,
        },
        "ths": ths_report,
    }))
}

pub fn spawn(ctx: BridgeCtx) {
    tauri::async_runtime::spawn(async move {
        let app = Router::new()
            .route("/health", get(health))
            .route("/mstoken", get(capture_mstoken))
            .route("/signal", get(signal))
            .route("/login-url", get(login_url))
            .route("/context", get(context))
            .layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(Any)
                    .allow_headers(Any),
            )
            .with_state(ctx);
        let bind = format!("127.0.0.1:{BRIDGE_PORT}");
        match tokio::net::TcpListener::bind(&bind).await {
            Ok(listener) => {
                log::info!("bridge listening on {bind}");
                if let Err(e) = axum::serve(listener, app).await {
                    log::error!("bridge serve: {e:#}");
                }
            }
            Err(e) => log::error!("bridge bind {bind} failed: {e:#}"),
        }
    });
}
