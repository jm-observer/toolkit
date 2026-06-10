//! 本地 HTTP 桥 — G10 web ↔ desktop 上下文交换。
//!
//! 切到 headless_chrome 后，桥**只剩"读"侧**：暴露 desktop 本机上下文给 G10 web。
//! 以前给抖音 login_hook.js 喂 msToken 的 /mstoken /signal 端点不再需要——CDP 直接
//! 拿全量 cookie 含 msToken。
//!
//! 路由：
//! - `GET /health`     — 探活
//! - `GET /login-url`  — douyin tab 当前 URL（轻量）
//! - `GET /context`    — 完整 desktop 上下文（douyin URL + cookies count + ths status）

use crate::browser::Session;
use crate::ths;
use axum::extract::State;
use axum::response::Json;
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

pub const BRIDGE_PORT: u16 = 28788;

#[derive(Clone)]
pub struct BridgeCtx {
    pub douyin: Arc<Session>,
    pub ths: Arc<Session>,
    pub workspace: PathBuf,
}

async fn health() -> &'static str {
    "bridge ok"
}

async fn login_url(State(ctx): State<BridgeCtx>) -> Json<Value> {
    let has_window = ctx.douyin.is_open();
    let url = if has_window {
        ctx.douyin.current_url()
    } else {
        None
    };
    Json(json!({ "has_window": has_window, "url": url }))
}

async fn context(State(ctx): State<BridgeCtx>) -> Json<Value> {
    let has_window = ctx.douyin.is_open();
    let url = if has_window {
        ctx.douyin.current_url()
    } else {
        None
    };
    let cookies_count = if has_window {
        ctx.douyin
            .tab()
            .and_then(|t| t.get_cookies().ok())
            .map(|c| c.len() as i64)
    } else {
        None
    };
    let ths_report = ths::status_report(&ctx.workspace);
    let ths_has_window = ctx.ths.is_open();
    Json(json!({
        "login": {
            "has_window": has_window,
            "url": url,
            "cookies_count": cookies_count,
        },
        "ths": {
            "has_window": ths_has_window,
            "report": ths_report,
        },
    }))
}

pub fn spawn(ctx: BridgeCtx) {
    tauri::async_runtime::spawn(async move {
        let app = Router::new()
            .route("/health", get(health))
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
