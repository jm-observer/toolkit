//! 本地 HTTP 桥 — 绕开 Tauri 远端 webview 的 invoke 鉴权。
//!
//! login 窗口加载的是 `https://www.douyin.com`（远端 origin），Tauri 2 默认不让远端
//! 页面调任何自定义 `#[tauri::command]`，把它配通要走 plugin-style permission 注册，繁琐。
//! 这里直接在 127.0.0.1:28788 起一个最小 axum 服务，hook 用 `fetch(.., {mode:'no-cors'})`
//! 喂 msToken；CORS 全开（仅本机可达，远端摸不到）。

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

async fn capture(
    State(state): State<Arc<UploaderState>>,
    Query(q): Query<TokenQuery>,
) -> &'static str {
    let trimmed = q.value.trim().to_string();
    if trimmed.len() < 16 {
        return "skip";
    }
    let mut slot = state.ms_token.lock().await;
    let changed = slot.as_deref() != Some(trimmed.as_str());
    if changed {
        log::info!("bridge captured new msToken (len={})", trimmed.len());
        *slot = Some(trimmed);
        // 触发下一 tick 必传
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
            .route("/mstoken", get(capture))
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
