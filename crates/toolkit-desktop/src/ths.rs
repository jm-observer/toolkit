//! 同花顺登录态采集 — 与 `stock-trade/ths/src/cookies.rs` 的 `CookieRecord` 兼容。
//!
//! 流程：用户点 UI 上「打开同花顺登录」→ desktop 起 label=`ths-login` 的 WebView 窗
//! 加载 `https://q.10jqka.com.cn/`，用户手动完成账号 + 滑块（**务必勾「记住我」**，否则
//! `ticket` 是 session cookie 关窗即失效）→ watcher 每 5s 读窗口 cookie，关键三件套齐了
//! 就以 THS 兼容格式落盘到 `<workspace>/ths/cookies.json`，THS 项目把它当 `DEFAULT_COOKIE_PATH`
//! 用就行。
//!
//! 落盘 dedup 走 cookie value SHA256，避免心跳/滑动刷新这种无关变更触发重写。

use anyhow::{anyhow, Context, Result};
use chrono::TimeZone;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::WebviewWindow;

pub const LOGIN_URL: &str = "https://q.10jqka.com.cn/";
pub const REQUIRED: &[&str] = &["ticket", "user", "userid"];

/// 与 stock-trade `ths::cookies::CookieRecord` 字段一一对应，serde 序列化兼容。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieRecord {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    /// Unix 秒；-1 表示 session cookie。
    pub expires: f64,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default)]
    pub secure: bool,
}

pub fn cookies_path(workspace: &Path) -> PathBuf {
    workspace.join("ths").join("cookies.json")
}

/// 从 login 窗口读 10jqka 域 cookie 并转换。
pub fn read_records(login: &WebviewWindow) -> Result<Vec<CookieRecord>> {
    let url: url::Url = LOGIN_URL.parse().context("parse ths login url")?;
    let cookies = login
        .cookies_for_url(url)
        .map_err(|e| anyhow!("cookies_for_url: {e}"))?;
    Ok(cookies
        .into_iter()
        .map(|c| {
            let expires = match c.expires() {
                Some(tauri::webview::cookie::Expiration::DateTime(dt)) => dt.unix_timestamp() as f64,
                _ => -1.0,
            };
            CookieRecord {
                name: c.name().to_string(),
                value: c.value().to_string(),
                domain: c.domain().unwrap_or("").to_string(),
                path: c.path().unwrap_or("/").to_string(),
                expires,
                http_only: c.http_only().unwrap_or(false),
                secure: c.secure().unwrap_or(false),
            }
        })
        .collect())
}

#[allow(dead_code)]
pub fn has_required(records: &[CookieRecord]) -> bool {
    REQUIRED
        .iter()
        .all(|name| records.iter().any(|r| r.name == *name))
}

pub fn missing_required(records: &[CookieRecord]) -> Vec<&'static str> {
    REQUIRED
        .iter()
        .copied()
        .filter(|name| !records.iter().any(|r| r.name == *name))
        .collect()
}

pub fn save(workspace: &Path, records: &[CookieRecord]) -> Result<PathBuf> {
    let path = cookies_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(records)?;
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub fn hash(records: &[CookieRecord]) -> String {
    let mut h = Sha256::new();
    for r in records {
        h.update(r.name.as_bytes());
        h.update(b"=");
        h.update(r.value.as_bytes());
        h.update(b";");
    }
    hex::encode(h.finalize())
}

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub exists: bool,
    pub count: i64,
    pub has_required: bool,
    pub missing: Vec<String>,
    pub ticket_expires_at: Option<String>,
    pub ticket_is_session: bool,
    pub path: String,
}

/// 读 cookies.json 给 UI / 主窗显示当前 THS 登录态状况。
pub fn status_report(workspace: &Path) -> StatusReport {
    let path = cookies_path(workspace);
    let path_str = path.display().to_string();
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => {
            return StatusReport {
                exists: false,
                count: 0,
                has_required: false,
                missing: REQUIRED.iter().map(|s| (*s).to_string()).collect(),
                ticket_expires_at: None,
                ticket_is_session: false,
                path: path_str,
            };
        }
    };
    let records: Vec<CookieRecord> = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            return StatusReport {
                exists: true,
                count: 0,
                has_required: false,
                missing: REQUIRED.iter().map(|s| (*s).to_string()).collect(),
                ticket_expires_at: None,
                ticket_is_session: false,
                path: path_str,
            };
        }
    };
    let missing = missing_required(&records);
    let ticket = records.iter().find(|r| r.name == "ticket");
    let (ticket_expires_at, ticket_is_session) = match ticket {
        Some(c) if c.expires > 0.0 => {
            let dt = chrono::Utc
                .timestamp_opt(c.expires as i64, 0)
                .single()
                .map(|d| d.to_rfc3339());
            (dt, false)
        }
        Some(_) => (None, true),
        None => (None, false),
    };
    StatusReport {
        exists: true,
        count: records.len() as i64,
        has_required: missing.is_empty(),
        missing: missing.into_iter().map(String::from).collect(),
        ticket_expires_at,
        ticket_is_session,
        path: path_str,
    }
}
