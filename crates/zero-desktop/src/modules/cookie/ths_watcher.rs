//! THS Chrome tab watcher：每 5s 通过 CDP 读 cookie，关键三件套齐 + 内容变化时落盘。

use super::ths;
use super::CookieState;
use custom_utils::trace::{self, SpanScope, SpanStatus, TraceContext};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

const POLL_SECS: u64 = 5;

#[derive(Default)]
pub struct ThsState {
    pub(crate) last_hash: Mutex<Option<String>>,
}

pub fn spawn(app: AppHandle, state: Arc<CookieState>) {
    tauri::async_runtime::spawn(async move {
        // 顶层 anchor span：仅 emit_start，长循环不 emit_end（进程退出时 trace-hub
        // 按 in-flight 处理，设计约定：后台循环不 emit_end）。
        let _loop_scope = trace::enabled().then(|| {
            let ctx = TraceContext::root();
            let scope = SpanScope::new(ctx, "ths_watcher_loop").with_summary(
                serde_json::json!({"service": "ths_watcher", "poll_secs": POLL_SECS}),
            );
            scope.emit_start();
            scope
        });
        let mut ticker = tokio::time::interval(Duration::from_secs(POLL_SECS));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            tick(&app, &state).await;
        }
    });
}

async fn tick(app: &AppHandle, state: &Arc<CookieState>) {
    if !state.ths_browser.is_open() {
        let _ = app.emit(
            "ths:status",
            serde_json::json!({ "state": "no_login_window" }),
        );
        return;
    }
    let Some(tab) = state.ths_browser.tab() else {
        return;
    };
    let records = match tokio::task::spawn_blocking(move || ths::read_records(&tab)).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            let _ = app.emit(
                "ths:status",
                serde_json::json!({ "state": "error", "error": e.to_string() }),
            );
            return;
        }
        Err(e) => {
            log::warn!("ths spawn_blocking: {e}");
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
                "hint": "请在登录窗里完成账号 + 滑块验证，务必勾「记住我」。",
            }),
        );
        return;
    }
    let hash = ths::hash(&records);
    {
        let mut last = state.ths.last_hash.lock().await;
        if last.as_deref() == Some(&hash) {
            let _ = app.emit(
                "ths:status",
                serde_json::json!({ "state": "unchanged", "count": records.len() }),
            );
            return;
        }
        *last = Some(hash);
    }
    match ths::save(&state.workspace, &records) {
        Ok(path) => {
            tracing::info!(target: "cookie", "ths cookies saved -> {}", path.display());
            let report = ths::status_report(&state.workspace);
            // 有实际变化时 emit 子 span，记录落盘事件。
            if trace::enabled() {
                let ctx = TraceContext::root();
                let scope =
                    SpanScope::new(ctx, "ths_cookie_saved").with_summary(serde_json::json!({
                        "count": records.len(),
                        "path": path.display().to_string(),
                    }));
                scope.emit_start();
                scope.emit_end(None, SpanStatus::Ok, None);
            }
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
            // 上传失败时 emit 子 span 记录错误。
            if trace::enabled() {
                let ctx = TraceContext::root();
                let scope = SpanScope::new(ctx, "ths_cookie_saved")
                    .with_summary(serde_json::json!({"error": format!("{e:#}")}));
                scope.emit_start();
                scope.emit_end(None, SpanStatus::Error(format!("{e:#}")), None);
            }
            let _ = app.emit(
                "ths:status",
                serde_json::json!({ "state": "error", "error": format!("{e:#}") }),
            );
        }
    }
}
