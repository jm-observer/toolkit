//! CLI → daemon 的 HTTP 客户端（设计 §CLI 与服务交互）。
//!
//! 长任务命令（submit/status/retry/cancel/reap/list-tasks/events/callback-flush）不再在
//! CLI 进程内直接操作文件，而是透传到 `douyin serve` daemon——daemon 是任务存储的唯一
//! 真相源，CLI 只引用 task_id。daemon 未运行时返回 `service_unavailable`（不静默降级为
//! 前台长任务，避免重新引入 Agent 调用超时）。
//!
//! daemon 地址：环境变量 `DOUYIN_DAEMON`，缺省 `http://127.0.0.1:8787`。
//!
//! 注意：daemon 拥有自己的 task_dir，因此这些命令的 `--task-dir` 不再生效（store 在 daemon
//! 侧）；submit 仍把 cookie/out/transcript 等 worker 输入路径作为 params 传给 daemon。

use serde_json::{json, Value};
use std::time::Duration;

/// daemon 基地址。
pub fn daemon_base() -> String {
    std::env::var("DOUYIN_DAEMON").unwrap_or_else(|_| "http://127.0.0.1:8787".to_string())
}

fn service_unavailable() -> Value {
    json!({
        "error": "douyin service 未运行",
        "error_kind": "service_unavailable",
        "hint": format!(
            "本机后台启动：`douyin daemon-start`；前台调试：`douyin serve`；G10：`systemctl --user start douyin`。\
             指向已有实例：设 DOUYIN_DAEMON=<url>。当前目标 {}",
            daemon_base()
        ),
    })
}

/// 健康探测当前 DOUYIN_DAEMON 目标。
pub async fn is_alive() -> bool {
    is_alive_at(&daemon_base()).await
}

/// 健康探测指定 base URL（daemon-start 用，避免 bind 端口与 DOUYIN_DAEMON 不一致时误判）。
pub async fn is_alive_at(base: &str) -> bool {
    let url = format!("{}/healthz", base.trim_end_matches('/'));
    reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn req(method: reqwest::Method, path: &str, body: Option<Value>) -> Value {
    let url = format!("{}{}", daemon_base(), path);
    let client = reqwest::Client::new();
    let mut rb = client.request(method, &url).timeout(Duration::from_secs(30));
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    match rb.send().await {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(v) => v,
            Err(e) => json!({ "error": format!("解析 daemon 响应失败: {e}"), "error_kind": "internal" }),
        },
        // 连接被拒 / DNS / 连不上 → daemon 没起。
        Err(e) if e.is_connect() => service_unavailable(),
        Err(e) => json!({ "error": format!("调 daemon 失败: {e}"), "error_kind": "network_error" }),
    }
}

pub async fn get(path: &str) -> Value {
    req(reqwest::Method::GET, path, None).await
}

pub async fn post(path: &str, body: Option<Value>) -> Value {
    req(reqwest::Method::POST, path, body).await
}

/// 入队任务：`POST /v1/jobs {kind, params}`。
pub async fn submit(kind: &str, params: Value) -> Value {
    post("/v1/jobs", Some(json!({ "kind": kind, "params": params }))).await
}
