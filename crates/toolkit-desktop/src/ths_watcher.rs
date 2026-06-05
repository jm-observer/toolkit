//! THS login 窗口 watcher：每 5s 读 cookie，关键三件套齐 + 内容变化时落盘。
//!
//! 与 uploader 的设计同形：无 ths-login 窗 → emit `no_login_window`；登录未完成 →
//! `waiting_login` + missing；齐了且 hash 变化 → 写 `<workspace>/ths/cookies.json` 并 emit
//! `saved`；hash 同上次 → `unchanged`。前端 `listen("ths:status")` 接所有状态。

use crate::ths;
use crate::AppCtx;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

const POLL_SECS: u64 = 5;

#[derive(Default)]
pub struct ThsState {
    pub(crate) last_hash: Mutex<Option<String>>,
}

pub fn spawn(app: AppHandle, ctx: AppCtx) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(POLL_SECS));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            tick(&app, &ctx).await;
        }
    });
}

async fn tick(app: &AppHandle, ctx: &AppCtx) {
    let Some(login) = app.get_webview_window("ths-login") else {
        let _ = app.emit(
            "ths:status",
            serde_json::json!({ "state": "no_login_window" }),
        );
        return;
    };
    let records = match ths::read_records(&login) {
        Ok(v) => v,
        Err(e) => {
            let _ = app.emit(
                "ths:status",
                serde_json::json!({ "state": "error", "error": e.to_string() }),
            );
            return;
        }
    };
    let missing = ths::missing_required(&records);
    if !missing.is_empty() {
        let _ = app.emit(
            "ths:status",
            serde_json::json!({
                "state": "waiting_login",
                "missing": missing,
                "have": records.len(),
                "hint": "请在登录窗里完成账号 + 滑块验证，务必勾「记住我」，否则 ticket 是 session cookie 关窗即失效。",
            }),
        );
        return;
    }
    let hash = ths::hash(&records);
    {
        let mut last = ctx.ths.last_hash.lock().await;
        if last.as_deref() == Some(&hash) {
            let _ = app.emit(
                "ths:status",
                serde_json::json!({ "state": "unchanged", "count": records.len() }),
            );
            return;
        }
        *last = Some(hash);
    }
    match ths::save(&ctx.workspace, &records) {
        Ok(path) => {
            log::info!("ths cookies saved -> {}", path.display());
            let report = ths::status_report(&ctx.workspace);
            let _ = app.emit(
                "ths:status",
                serde_json::json!({
                    "state": "saved",
                    "count": records.len(),
                    "path": path.display().to_string(),
                    "report": report,
                }),
            );
        }
        Err(e) => {
            log::warn!("ths save failed: {e:#}");
            let _ = app.emit(
                "ths:status",
                serde_json::json!({ "state": "error", "error": format!("{e:#}") }),
            );
        }
    }
}
