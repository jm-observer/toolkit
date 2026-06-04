//! 后台轮询 WebView2 cookie store；登录态完整时打包 POST 到 toolkit-server。
//!
//! 流程：
//! 1. 每 `POLL_SECS` 秒 `webview.cookies_for_url("https://www.douyin.com")` 取 cookies；
//! 2. 必需字段（msToken / ttwid / sessionid_ss）齐全才视为登录态；
//! 3. 对 cookie 串算 SHA256，与上次相同则跳过（避免重复上传同一份）；
//! 4. 拼成 `k=v; k=v` header 串，POST `<server>/api/browser/cookie`，body 同
//!    `extension-contract.md §四.3`：`{session_id, raw_header}`；
//! 5. 上传结果写入 `state.db` uploads 表，并通过事件通知前端。

use crate::workspace;
use crate::AppCtx;
use crate::config;
use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tokio::sync::Mutex;

const POLL_SECS: u64 = 5;
const TARGET_URL: &str = "https://www.douyin.com";
/// 判定「登录态可上传」的核心字段。三个都齐才上传：
/// - `sessionid_ss` / `ttwid`：登录态
/// - `msToken`：抖音前端 SDK 动态写入；缺它 G10 业务接口（DouyinClient::from_cookies）直接 cookie_missing，
///   所以宁可不传不让 server 拿到不完整 cookie 覆盖旧的好版本。
const REQUIRED: &[&str] = &["ttwid", "sessionid_ss", "msToken"];

#[derive(Default)]
pub struct UploaderState {
    pub(crate) last_hash: Mutex<Option<String>>,
    session_id: Mutex<String>,
    /// JS hook 从抖音 API URL 里抓到的最新 msToken。
    pub(crate) ms_token: Mutex<Option<String>>,
}

impl UploaderState {
    pub async fn session_id(&self) -> String {
        let mut s = self.session_id.lock().await;
        if s.is_empty() {
            *s = format!("desktop-{}", chrono::Utc::now().timestamp_millis());
        }
        s.clone()
    }
}

pub fn spawn(app: AppHandle, ctx: AppCtx) {
    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client");
        let mut ticker = tokio::time::interval(Duration::from_secs(POLL_SECS));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = tick(&app, &ctx, &client).await {
                log::warn!("uploader tick: {e:#}");
                let _ = app.emit("uploader:status", json_err(&e.to_string()));
            }
        }
    });
}

async fn tick(app: &AppHandle, ctx: &AppCtx, client: &reqwest::Client) -> Result<()> {
    let settings = config::load(&workspace::config_path(&ctx.workspace));
    let Some(endpoint) = settings.cookie_endpoint() else {
        let _ = app.emit(
            "uploader:status",
            serde_json::json!({ "state": "unconfigured", "hint": "填 server base 并保存" }),
        );
        return Ok(());
    };
    let Some(login) = app.get_webview_window("login") else {
        let _ = app.emit(
            "uploader:status",
            serde_json::json!({ "state": "no_login_window", "hint": "点「打开抖音登录」并保持窗口" }),
        );
        return Ok(());
    };
    let mut pairs = read_cookies(&login).await?;
    // 把 JS hook 抓到的 msToken 合进来（抖音不再写 cookie，仅出现在 API URL 上）。
    if let Some(tok) = ctx.uploader.ms_token.lock().await.clone() {
        if !pairs.iter().any(|(n, _)| n == "msToken") {
            pairs.push(("msToken".to_string(), tok));
        }
    }
    let missing: Vec<&str> = REQUIRED
        .iter()
        .copied()
        .filter(|k| !pairs.iter().any(|(n, _)| n == k))
        .collect();
    if !missing.is_empty() {
        let hint = if missing == ["msToken"] {
            "已登录但缺 msToken — 抖音前端 SDK 还没把它写入 cookie。请在登录窗里滚动首页或点开一个视频几秒，桌面端会自动检测到并上传。"
        } else if missing.iter().any(|k| *k == "sessionid_ss" || *k == "ttwid") {
            "未检测到登录态：请在登录窗里完成抖音账号登录。"
        } else {
            "cookie 不完整，等待补齐后再上传。"
        };
        let _ = app.emit(
            "uploader:status",
            serde_json::json!({
                "state": "waiting_login",
                "missing": missing,
                "have": pairs.len(),
                "hint": hint,
            }),
        );
        return Ok(());
    }
    let raw_header = build_header(&pairs);
    let hash = sha256_hex(&raw_header);
    {
        let mut last = ctx.uploader.last_hash.lock().await;
        if last.as_deref() == Some(&hash) {
            let _ = app.emit(
                "uploader:status",
                serde_json::json!({ "state": "unchanged", "fields": pairs.len() }),
            );
            return Ok(());
        }
        *last = Some(hash.clone());
    }
    let session_id = ctx.uploader.session_id().await;
    let now = chrono::Utc::now().to_rfc3339();
    let _ = ctx.db.upsert_session(&session_id, &now);

    let body = serde_json::json!({ "session_id": session_id, "raw_header": raw_header });
    let mut req = client.post(&endpoint).json(&body);
    if let Some(tok) = settings.auth_token.as_deref().filter(|s| !s.is_empty()) {
        req = req.bearer_auth(tok);
    }
    let send_result = req.send().await;
    match send_result {
        Err(e) => {
            let _ = ctx
                .db
                .record_upload(&now, &hash, pairs.len() as i64, false, None, Some(&e.to_string()));
            return Err(anyhow!("POST cookie: {e}"));
        }
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                let err_msg = format!("server {status}: {text}");
                let _ = ctx.db.record_upload(
                    &now,
                    &hash,
                    pairs.len() as i64,
                    false,
                    Some(&text),
                    Some(&err_msg),
                );
                return Err(anyhow!(err_msg));
            }
            let _ = ctx.db.record_upload(
                &now,
                &hash,
                pairs.len() as i64,
                true,
                Some(&text),
                None,
            );
            // 持久化 last_uploaded_at 到 config.json，供下次启动 UI 显示。
            let mut s2 = settings.clone();
            s2.last_uploaded_at = Some(now.clone());
            if let Err(e) = config::save(&workspace::config_path(&ctx.workspace), &s2) {
                log::warn!("save settings after upload: {e:#}");
            }
            let _ = app.emit(
                "uploader:status",
                serde_json::json!({
                    "state": "uploaded",
                    "fields": pairs.len(),
                    "endpoint": endpoint,
                    "at": now,
                    "server_response": text,
                }),
            );
        }
    }
    Ok(())
}

async fn read_cookies(login: &WebviewWindow) -> Result<Vec<(String, String)>> {
    let url: url::Url = TARGET_URL.parse().context("parse target url")?;
    let cookies = login
        .cookies_for_url(url)
        .map_err(|e| anyhow!("cookies_for_url: {e}"))?;
    Ok(cookies
        .into_iter()
        .map(|c| (c.name().to_string(), c.value().to_string()))
        .collect())
}

fn build_header(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

fn json_err(msg: &str) -> serde_json::Value {
    serde_json::json!({ "state": "error", "error": msg })
}

// 让 Arc<UploaderState> 仍可通过 ctx.uploader 拿到 — 见 main.rs::AppCtx。
#[allow(dead_code)]
pub type UploaderRef = Arc<UploaderState>;
