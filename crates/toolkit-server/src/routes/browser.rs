//! Chrome 扩展 ↔ server 三个 HTTP endpoint。
//! 协议见 docs/toolkit-rfc/2026-06-04-initial-skeleton/extension-contract.md §四。

use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::post;
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use toolkit_core::{classify_url, now_iso8601, UrlMatch};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/hello", post(hello))
        .route("/url", post(url))
        .route("/cookie", post(cookie))
}

#[derive(Debug, Deserialize)]
struct HelloBody {
    session_id: String,
    #[serde(default)]
    user_agent: Option<String>,
    #[serde(default)]
    extension_version: Option<String>,
}

async fn hello(
    State(s): State<AppState>,
    Json(body): Json<HelloBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let now = now_iso8601();
    let conn = s.pool.get().map_err(internal)?;
    conn.execute(
        "INSERT INTO browser_sessions(session_id, user_agent, first_seen, last_seen)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(session_id) DO UPDATE SET
           user_agent = excluded.user_agent,
           last_seen  = excluded.last_seen",
        params![body.session_id, body.user_agent, now],
    )
    .map_err(internal)?;
    let _ = body.extension_version;
    Ok(Json(json!({
        "server_version": env!("CARGO_PKG_VERSION"),
        "accepted_at": now,
    })))
}

#[derive(Debug, Deserialize)]
struct UrlBody {
    session_id: String,
    #[serde(default)]
    tab_id: Option<i64>,
    url: String,
    #[serde(default)]
    title: Option<String>,
}

async fn url(
    State(s): State<AppState>,
    Json(body): Json<UrlBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let now = now_iso8601();
    let conn = s.pool.get().map_err(internal)?;
    conn.execute(
        "INSERT INTO browser_sessions(session_id, first_seen, last_seen, current_url)
         VALUES (?1, ?2, ?2, ?3)
         ON CONFLICT(session_id) DO UPDATE SET
           last_seen   = excluded.last_seen,
           current_url = excluded.current_url",
        params![body.session_id, now, body.url],
    )
    .map_err(internal)?;
    let _ = (body.tab_id, body.title);

    let m = classify_url(&body.url);
    let matched = match &m {
        UrlMatch::CreatorHome { .. } => Some("creator_home"),
        UrlMatch::CreatorHomeShort { .. } => Some("creator_home_short"),
        UrlMatch::Work { .. } => Some("work"),
        UrlMatch::Search => Some("search"),
        UrlMatch::None => None,
    };
    Ok(Json(json!({
        "matched": matched,
        "detail": m,
    })))
}

#[derive(Debug, Deserialize)]
struct CookieBody {
    #[serde(default)]
    session_id: Option<String>,
    raw_header: String,
    #[serde(default)]
    parsed: Option<Value>,
}

async fn cookie(
    State(s): State<AppState>,
    Json(body): Json<CookieBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let now = now_iso8601();
    let parsed = body
        .parsed
        .unwrap_or_else(|| parse_cookie_header(&body.raw_header));
    let fields_count = parsed.as_object().map(|m| m.len() as i64).unwrap_or(0);
    let has_required: Vec<String> = ["msToken", "ttwid", "sessionid_ss"]
        .iter()
        .filter(|k| {
            parsed
                .as_object()
                .map(|m| m.contains_key(**k))
                .unwrap_or(false)
        })
        .map(|s| s.to_string())
        .collect();

    let conn = s.pool.get().map_err(internal)?;
    conn.execute(
        "INSERT INTO cookies(id, raw, parsed, captured_at, status)
         VALUES (1, ?1, ?2, ?3, 'unknown')
         ON CONFLICT(id) DO UPDATE SET
           raw = excluded.raw,
           parsed = excluded.parsed,
           captured_at = excluded.captured_at,
           status = 'unknown'",
        params![body.raw_header, parsed.to_string(), now],
    )
    .map_err(internal)?;

    // 同步写一份 douyin 兼容格式（cookies.json），让 douyin crate 可直接读用。失败仅警告。
    if let Err(e) =
        crate::douyin_mod::cookie_bridge::write_from_raw_header(&s.data_dir, &body.raw_header).await
    {
        log::warn!("cookie bridge to douyin failed: {e:#}");
    }

    let _ = body.session_id;
    Ok(Json(json!({
        "accepted": true,
        "fields_count": fields_count,
        "has_required": has_required,
    })))
}

fn parse_cookie_header(raw: &str) -> Value {
    let mut m = serde_json::Map::new();
    for kv in raw.split(';') {
        let kv = kv.trim();
        if let Some((k, v)) = kv.split_once('=') {
            m.insert(k.trim().to_string(), Value::String(v.trim().to_string()));
        }
    }
    Value::Object(m)
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": format!("{e}")})),
    )
}
