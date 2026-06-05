//! 本地 HTTP 桥 — login 远端窗口 ↔ desktop 后端的统一数据通道。
//!
//! 设计取舍：login 窗加载 `https://www.douyin.com`（远端 origin），Tauri 2 默认不让远端
//! 页面调任何 `#[tauri::command]`，绕开方案是注册 plugin-style permission，繁琐易错。
//! 这里直接在 `127.0.0.1:BRIDGE_PORT` 起最小 axum，远端 origin 摸不到本机端口（私网/同源
//! 都不会路由到此），加 CORS 全开仅为让 hook fetch 不报错。
//!
//! 路由（全 GET 避免 CORS 预检）：
//! - `/health`           — 探活
//! - `/mstoken?value=`   — msToken 专路，写 `UploaderState.ms_token` 并触发立即重传
//! - `/signal?name=&value=` — 通用槽，未来加 JS→Rust 信号（当前 URL / 账号信息 / 签名等）
//!                          只需在本文件加 match 分支，不动 hook 协议外形。

use crate::uploader::UploaderState;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

pub const BRIDGE_PORT: u16 = 28788;

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
    State(state): State<Arc<UploaderState>>,
    Query(q): Query<TokenQuery>,
) -> &'static str {
    record_ms_token(&state, q.value).await
}

async fn signal(
    State(state): State<Arc<UploaderState>>,
    Query(q): Query<SignalQuery>,
) -> &'static str {
    match q.name.as_str() {
        "mstoken" => record_ms_token(&state, q.value).await,
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

pub fn spawn(state: Arc<UploaderState>) {
    tauri::async_runtime::spawn(async move {
        let app = Router::new()
            .route("/health", get(health))
            .route("/mstoken", get(capture_mstoken))
            .route("/signal", get(signal))
            .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
            .with_state(state);
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
