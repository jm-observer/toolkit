//! `/api/web/douyin/*` 路由。
//!
//! 同步查询（creator / works / tags / filter / cookie_status）直接转发 douyin 库；
//! 长任务（sync_works / download / transcribe）走 toolkit-tasks 提交。

use crate::douyin_mod::paths::DouyinPaths;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use toolkit_core::now_iso8601;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/creator", get(creator))
        .route("/creators", get(list_creators).post(track_creator))
        .route("/works", get(works_list))
        .route("/works_saved", get(works_saved))
        .route("/tags", get(tags))
        .route("/filter", get(filter))
        .route("/cookie_status", get(cookie_status))
        .route("/sync_works", post(sync_works))
        .route("/download", post(download))
        .route("/transcribe", post(transcribe))
        .route("/refine", post(refine))
        .route("/pipeline", post(pipeline))
        .route("/kb_publish", post(kb_publish))
}

// ---------------- 同步查询 ----------------

#[derive(Debug, Deserialize)]
struct CreatorQuery {
    handle: String,
}

async fn creator(
    State(s): State<AppState>,
    Query(q): Query<CreatorQuery>,
) -> Result<Json<Value>, ApiError> {
    let paths = DouyinPaths::new(&s.workspace);
    paths.ensure_dirs().map_err(internal)?;
    let v = douyin::run_resolve_user(&paths.cookie_file, &q.handle)
        .await
        .map_err(internal)?;
    Ok(Json(v))
}

#[derive(Debug, Deserialize)]
struct WorksQuery {
    handle: String,
    #[serde(default = "default_max_pages")]
    max_pages: usize,
}

fn default_max_pages() -> usize {
    60
}

async fn works_list(
    State(s): State<AppState>,
    Query(q): Query<WorksQuery>,
) -> Result<Json<Value>, ApiError> {
    let paths = DouyinPaths::new(&s.workspace);
    paths.ensure_dirs().map_err(internal)?;
    let v = douyin::run_list_works(&paths.cookie_file, &q.handle, q.max_pages)
        .await
        .map_err(internal)?;
    Ok(Json(v))
}

#[derive(Debug, Deserialize)]
struct TagsQuery {
    unique_id: String,
}

async fn tags(
    State(s): State<AppState>,
    Query(q): Query<TagsQuery>,
) -> Result<Json<Value>, ApiError> {
    let paths = DouyinPaths::new(&s.workspace);
    let v = douyin::run_list_tags(&paths.works_dir, &q.unique_id).map_err(internal)?;
    Ok(Json(v))
}

/// 列某博主已存盘作品 + 每条处理状态（下载/转写/整理）。供 web 工作台勾选操作。
async fn works_saved(
    State(s): State<AppState>,
    Query(q): Query<TagsQuery>,
) -> Result<Json<Value>, ApiError> {
    let paths = DouyinPaths::new(&s.workspace);
    let v = douyin::run_list_saved_works(
        &paths.works_dir,
        &paths.out_dir,
        &paths.transcript_dir,
        &paths.refined_dir,
        &q.unique_id,
    )
    .map_err(internal)?;
    Ok(Json(v))
}

#[derive(Debug, Deserialize)]
struct FilterQuery {
    unique_id: String,
    tags: String,
    #[serde(default = "default_match")]
    r#match: String,
}

fn default_match() -> String {
    "all".to_string()
}

async fn filter(
    State(s): State<AppState>,
    Query(q): Query<FilterQuery>,
) -> Result<Json<Value>, ApiError> {
    let paths = DouyinPaths::new(&s.workspace);
    let tags: Vec<String> = q
        .tags
        .split([',', ' '])
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let match_all = q.r#match.eq_ignore_ascii_case("all");
    let v = douyin::run_filter_works(&paths.works_dir, &q.unique_id, &tags, match_all)
        .map_err(internal)?;
    Ok(Json(v))
}

// ---------------- 博主追踪（desktop 推送 → web 列表） ----------------

#[derive(Debug, Deserialize)]
struct TrackBody {
    handle: String,
}

/// 解析 handle → 拿博主资料 → upsert 到 creators 表。供 desktop 一键收录。
async fn track_creator(
    State(s): State<AppState>,
    Json(body): Json<TrackBody>,
) -> Result<Json<Value>, ApiError> {
    let paths = DouyinPaths::new(&s.workspace);
    paths.ensure_dirs().map_err(internal)?;
    let v = douyin::run_resolve_user(&paths.cookie_file, &body.handle)
        .await
        .map_err(internal)?;
    if v.get("error").is_some() {
        return Err(bad_request(
            v.get("error")
                .and_then(|x| x.as_str())
                .unwrap_or("resolve failed"),
        ));
    }
    let sec_uid = v
        .get("sec_uid")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let unique_id = v
        .get("unique_id")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let nickname = v
        .get("nickname")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let signature = v
        .get("signature")
        .and_then(|x| x.as_str())
        .map(String::from);
    let aweme_count = v.get("aweme_count").and_then(|x| x.as_i64());
    let follower_count = v.get("follower_count").and_then(|x| x.as_i64());
    if sec_uid.is_empty() || unique_id.is_empty() {
        return Err(bad_request("resolve 返回缺 sec_uid 或 unique_id"));
    }
    let now = now_iso8601();
    let conn = s.pool.get().map_err(internal)?;
    conn.execute(
        "INSERT INTO creators(unique_id, sec_uid, nickname, signature, follower_count, aweme_count, verified, raw, added_at, last_synced_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, ?8)
         ON CONFLICT(unique_id) DO UPDATE SET
           sec_uid        = excluded.sec_uid,
           nickname       = excluded.nickname,
           signature      = excluded.signature,
           follower_count = excluded.follower_count,
           aweme_count    = excluded.aweme_count,
           raw            = excluded.raw,
           last_synced_at = excluded.last_synced_at",
        params![
            unique_id, sec_uid, nickname, signature, follower_count, aweme_count,
            v.to_string(), now,
        ],
    )
    .map_err(internal)?;
    Ok(Json(json!({
        "tracked": true,
        "unique_id": unique_id,
        "sec_uid": sec_uid,
        "nickname": nickname,
        "aweme_count": aweme_count,
        "source_handle": body.handle,
        "creator": v,
    })))
}

#[derive(Debug, Deserialize)]
struct ListCreatorsQuery {
    #[serde(default = "default_creators_limit")]
    limit: i64,
}
fn default_creators_limit() -> i64 {
    50
}

async fn list_creators(
    State(s): State<AppState>,
    Query(q): Query<ListCreatorsQuery>,
) -> Result<Json<Value>, ApiError> {
    let conn = s.pool.get().map_err(internal)?;
    let mut stmt = conn
        .prepare(
            "SELECT unique_id, sec_uid, nickname, signature, follower_count, aweme_count,
                    added_at, last_synced_at
             FROM creators ORDER BY last_synced_at DESC, added_at DESC LIMIT ?1",
        )
        .map_err(internal)?;
    let rows = stmt
        .query_map([q.limit], |r| {
            Ok(json!({
                "unique_id": r.get::<_, String>(0)?,
                "sec_uid": r.get::<_, String>(1)?,
                "nickname": r.get::<_, String>(2)?,
                "signature": r.get::<_, Option<String>>(3)?,
                "follower_count": r.get::<_, Option<i64>>(4)?,
                "aweme_count": r.get::<_, Option<i64>>(5)?,
                "added_at": r.get::<_, String>(6)?,
                "last_synced_at": r.get::<_, Option<String>>(7)?,
            }))
        })
        .map_err(internal)?;
    let creators: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
    Ok(Json(
        json!({ "count": creators.len(), "creators": creators }),
    ))
}

async fn cookie_status(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let paths = DouyinPaths::new(&s.workspace);
    paths.ensure_dirs().map_err(internal)?;
    let v = douyin::run_cookie_status(&paths.cookie_file)
        .await
        .map_err(internal)?;
    Ok(Json(v))
}

// ---------------- 长任务提交 ----------------

async fn sync_works(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    submit_kind(&s, &headers, "douyin_list_works", body)
}

async fn download(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    submit_kind(&s, &headers, "douyin_download", body)
}

async fn transcribe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    submit_kind(&s, &headers, "douyin_transcribe", body)
}

async fn refine(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    submit_kind(&s, &headers, "douyin_text_refine", body)
}

async fn pipeline(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    submit_kind(&s, &headers, "douyin_pipeline", body)
}

fn submit_kind(
    s: &AppState,
    headers: &HeaderMap,
    kind: &str,
    input: Value,
) -> Result<Json<Value>, ApiError> {
    // 透传入站 W3C traceparent，让抖音长任务接入上游同一条 trace。
    let trace_parent = custom_utils::trace::extract_traceparent(|name| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|str| str.to_string())
    });
    let task_id = toolkit_tasks::submit(
        &s.registry,
        &s.pool,
        &s.workspace,
        kind,
        input,
        None,
        trace_parent,
    )
    .map_err(bad_request)?;
    Ok(Json(json!({ "task_id": task_id, "kind": kind })))
}

// ---------------- 同步知识库录入 ----------------

#[derive(Debug, Deserialize)]
struct KbPublishBody {
    unique_id: String,
    #[serde(default)]
    only_ids: Vec<String>,
}

async fn kb_publish(
    State(s): State<AppState>,
    Json(body): Json<KbPublishBody>,
) -> Result<Json<Value>, ApiError> {
    let paths = DouyinPaths::new(&s.workspace);
    paths.ensure_dirs().map_err(internal)?;
    let v = douyin::run_publish_knowledge(
        &paths.works_dir,
        &paths.knowledge_dir,
        &paths.transcript_dir,
        &paths.refined_dir,
        &body.unique_id,
        &body.only_ids,
    )
    .map_err(internal)?;
    Ok(Json(v))
}

// ---------------- 错误响应 ----------------

type ApiError = (StatusCode, Json<Value>);

fn internal<E: std::fmt::Display>(e: E) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": format!("{e}") })),
    )
}

fn bad_request<E: std::fmt::Display>(e: E) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": format!("{e}") })),
    )
}
