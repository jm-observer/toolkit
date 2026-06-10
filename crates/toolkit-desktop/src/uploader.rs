//! 后台轮询 CDP 控制的 Chrome 子进程 cookie store；登录态完整时打包 POST 到 toolkit-server。
//!
//! 切到 headless_chrome 后流程简化：
//! 1. 每 `POLL_SECS` 秒在 `spawn_blocking` 里调 `tab.get_cookies()`，直接读 cookie store
//!    （不再需要 JS hook / 桥）
//! 2. 登录态字段（ttwid / sessionid_ss）齐全才视为登录态；msToken 非必需（见 REQUIRED 注释）
//! 3. SHA256 dedup
//! 4. POST `<server>/api/browser/cookie`，body `{session_id, raw_header}`
//! 5. 落 state.db + 事件通知前端

use crate::config;
use crate::workspace;
use crate::AppCtx;
use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

const POLL_SECS: u64 = 5;
// msToken 非必需（实测 2026-06-10：空 msToken 下 profile/list/detail/self_info 返回数据
// 一致、不触发风控；抖音新登录 profile 本就常不写 msToken cookie）。只认登录态字段。
const REQUIRED: &[&str] = &["ttwid", "sessionid_ss"];

#[derive(Default)]
pub struct UploaderState {
    pub(crate) last_hash: Mutex<Option<String>>,
    session_id: Mutex<String>,
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
    if !ctx.douyin_browser.is_open() {
        let _ = app.emit(
            "uploader:status",
            serde_json::json!({
                "state": "no_login_window",
                "hint": "点「抖音登录」打开 Chrome",
            }),
        );
        return Ok(());
    }

    let pairs = read_cookies(ctx).await?;
    let missing: Vec<&str> = REQUIRED
        .iter()
        .copied()
        .filter(|k| !pairs.iter().any(|(n, _)| n == k))
        .collect();
    if !missing.is_empty() {
        // msToken 已不在 REQUIRED（非必需）。缺的只会是登录态字段。
        let hint = "未检测到登录态：请在登录窗里完成抖音账号登录。";
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
    match req.send().await {
        Err(e) => {
            let _ = ctx.db.record_upload(
                &now,
                &hash,
                pairs.len() as i64,
                false,
                None,
                Some(&e.to_string()),
            );
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
            let _ = ctx
                .db
                .record_upload(&now, &hash, pairs.len() as i64, true, Some(&text), None);
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

/// 通过 CDP 拿 douyin tab 的全量 cookies。`tab.get_cookies()` 是同步阻塞 IO，扔进
/// spawn_blocking 不阻塞 tokio runtime。
async fn read_cookies(ctx: &AppCtx) -> Result<Vec<(String, String)>> {
    let Some(tab) = ctx.douyin_browser.tab() else {
        return Err(anyhow!("douyin browser not open"));
    };
    let mut pairs = tokio::task::spawn_blocking(move || -> Result<Vec<(String, String)>> {
        let cookies = tab.get_cookies().map_err(|e| anyhow!("get_cookies: {e}"))?;
        Ok(cookies.into_iter().map(|c| (c.name, c.value)).collect())
    })
    .await
    .map_err(|e| anyhow!("spawn_blocking: {e}"))??;
    // cookie store 里没有 msToken 时（新登录 profile 常如此），用从网络请求 harvest 到的
    // 兜底补上。msToken 非必需，纯属锦上添花。
    if !pairs.iter().any(|(n, _)| n == "msToken") {
        if let Some(tok) = ctx.douyin_browser.harvested_ms_token() {
            pairs.push(("msToken".to_string(), tok));
        }
    }
    Ok(pairs)
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
