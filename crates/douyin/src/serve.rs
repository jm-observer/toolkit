//! 常驻 daemon + 本机 HTTP API（设计 §7.2 完整档）。
//!
//! `douyin serve --bind 127.0.0.1:8787` 起一个 axum 服务，提供：
//! - **自动恢复**：启动时 + 每 tick 跑一轮 `run_maintenance`（reap 三类 stale 任务 +
//!   flush 未送达 callback），替代手动 `*-reap` / `callback-flush`。
//! - **HTTP API**：跨三类任务的查询 / 控制入口，供 Web 模块、Agent 包装层、运维复用。
//!
//! 安全：默认只监听 127.0.0.1（绑定地址由 `--bind` 决定，不展示 cookie/凭据原文）。
//!
//! 注意：本 MVP 中 daemon 与「CLI 直接 spawn worker」并存——daemon 是额外的 HTTP 访问面
//! 与自动维护进程，CLI submit 仍可独立工作。后续可把 CLI 改为透传 daemon（设计 §CLI 与服务）。

use anyhow::{Context, Result};
use axum::{
    extract::{Path as AxPath, Query, State},
    response::Html,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
struct AppState {
    task_dir: Arc<PathBuf>,
    stale_secs: i64,
}

/// 启动 daemon：先跑一轮维护，再起后台定时维护 + HTTP 服务（阻塞直到进程退出）。
pub async fn run(task_dir: PathBuf, bind: String, tick_secs: u64, stale_secs: i64) -> Result<()> {
    // 启动维护：把上次进程残留的 stale 任务恢复、补发漏掉的 callback。
    match crate::run_maintenance(&task_dir, stale_secs).await {
        Ok(v) => log::info!("[serve] startup maintenance: {v}"),
        Err(e) => log::warn!("[serve] startup maintenance failed: {e}"),
    }

    // 后台定时维护循环。
    let tick_dir = task_dir.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(tick_secs.max(1)));
        interval.tick().await; // 首拍立即返回，跳过（启动已手动跑过一轮）
        loop {
            interval.tick().await;
            match crate::run_maintenance(&tick_dir, stale_secs).await {
                Ok(v) => log::debug!("[serve] tick maintenance: {v}"),
                Err(e) => log::warn!("[serve] tick maintenance failed: {e}"),
            }
        }
    });

    let state = AppState {
        task_dir: Arc::new(task_dir),
        stale_secs,
    };
    let app = Router::new()
        .route("/", get(dashboard))
        .route("/healthz", get(healthz))
        .route("/v1/jobs", post(submit_job))
        .route("/v1/tasks", get(list_tasks))
        .route("/v1/tasks/{task_id}", get(get_task))
        .route("/v1/tasks/{task_id}/events", get(get_events))
        .route("/v1/tasks/{task_id}/retry", post(retry_task))
        .route("/v1/tasks/{task_id}/cancel", post(cancel_task))
        .route("/v1/callbacks/flush", post(flush_callbacks))
        .route("/v1/maintenance/run", post(run_maintenance_now))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("绑定 {bind}"))?;
    log::info!("[serve] douyin daemon listening on http://{bind}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum serve")?;
    Ok(())
}

/// Ctrl-C 优雅退出。
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    log::info!("[serve] shutdown signal received");
}

fn err_json(e: anyhow::Error) -> Value {
    json!({ "error": e.to_string(), "error_kind": "internal" })
}

/// 极简运维面板（设计 §Web 第一版：HTML + JS 轮询 /v1/tasks，无新依赖）。
async fn dashboard() -> Html<&'static str> {
    Html(include_str!("dashboard.html"))
}

async fn healthz() -> Json<Value> {
    Json(json!({ "ok": true, "service": "douyin" }))
}

#[derive(serde::Deserialize)]
struct SubmitReq {
    kind: String,
    #[serde(default)]
    params: Value,
}

async fn submit_job(State(s): State<AppState>, Json(req): Json<SubmitReq>) -> Json<Value> {
    Json(
        crate::run_submit_job(&s.task_dir, &req.kind, &req.params)
            .await
            .unwrap_or_else(err_json),
    )
}

async fn list_tasks(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    let state = q.get("state").map(String::as_str);
    Json(crate::run_list_tasks(&s.task_dir, state).unwrap_or_else(err_json))
}

async fn get_task(State(s): State<AppState>, AxPath(task_id): AxPath<String>) -> Json<Value> {
    Json(crate::run_task_status(&s.task_dir, &task_id).unwrap_or_else(err_json))
}

async fn get_events(State(s): State<AppState>, AxPath(task_id): AxPath<String>) -> Json<Value> {
    Json(crate::run_events(&s.task_dir, &task_id).unwrap_or_else(err_json))
}

async fn retry_task(State(s): State<AppState>, AxPath(task_id): AxPath<String>) -> Json<Value> {
    Json(crate::run_task_retry(&s.task_dir, &task_id).unwrap_or_else(err_json))
}

async fn cancel_task(State(s): State<AppState>, AxPath(task_id): AxPath<String>) -> Json<Value> {
    Json(crate::run_task_cancel(&s.task_dir, &task_id).unwrap_or_else(err_json))
}

async fn flush_callbacks(State(s): State<AppState>) -> Json<Value> {
    Json(crate::run_callback_flush(&s.task_dir).await.unwrap_or_else(err_json))
}

async fn run_maintenance_now(State(s): State<AppState>) -> Json<Value> {
    Json(
        crate::run_maintenance(&s.task_dir, s.stale_secs)
            .await
            .unwrap_or_else(err_json),
    )
}
