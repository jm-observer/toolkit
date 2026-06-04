//! `/api/web/douyin/*` 路由。
//!
//! 同步查询（creator / works / tags / filter / cookie_status）直接转发 douyin 库；
//! 长任务（sync_works / download / transcribe）走 toolkit-tasks 提交。

use crate::douyin_mod::paths::DouyinPaths;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/creator", get(creator))
        .route("/works", get(works_list))
        .route("/tags", get(tags))
        .route("/filter", get(filter))
        .route("/cookie_status", get(cookie_status))
        .route("/sync_works", post(sync_works))
        .route("/download", post(download))
        .route("/transcribe", post(transcribe))
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
    let paths = DouyinPaths::new(&s.data_dir);
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
    let paths = DouyinPaths::new(&s.data_dir);
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
    let paths = DouyinPaths::new(&s.data_dir);
    let v = douyin::run_list_tags(&paths.works_dir, &q.unique_id).map_err(internal)?;
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
    let paths = DouyinPaths::new(&s.data_dir);
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

async fn cookie_status(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let paths = DouyinPaths::new(&s.data_dir);
    paths.ensure_dirs().map_err(internal)?;
    let v = douyin::run_cookie_status(&paths.cookie_file)
        .await
        .map_err(internal)?;
    Ok(Json(v))
}

// ---------------- 长任务提交 ----------------

async fn sync_works(
    State(s): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    submit_kind(&s, "douyin_list_works", body)
}

async fn download(
    State(s): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    submit_kind(&s, "douyin_download", body)
}

async fn transcribe(
    State(s): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    submit_kind(&s, "douyin_transcribe", body)
}

fn submit_kind(s: &AppState, kind: &str, input: Value) -> Result<Json<Value>, ApiError> {
    let task_id = toolkit_tasks::submit(&s.registry, &s.pool, &s.data_dir, kind, input, None)
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
    let paths = DouyinPaths::new(&s.data_dir);
    paths.ensure_dirs().map_err(internal)?;
    let v = douyin::run_publish_knowledge(
        &paths.works_dir,
        &paths.knowledge_dir,
        &paths.transcript_dir,
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
