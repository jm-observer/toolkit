use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

/// 从入站请求头解析 W3C `traceparent`，得到上游当前 span 的上下文（供任务接入同一条
/// trace）。无头 / 格式非法返回 None。
fn trace_ctx_from_headers(headers: &HeaderMap) -> Option<toolkit_tasks::TraceContext> {
    custom_utils::trace::extract_traceparent(|name| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    })
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/tasks", get(list_tasks).post(submit_task))
        .route("/tasks/{task_id}", get(get_task))
        .route("/codeloop/sessions", get(codeloop_sessions))
        .route(
            "/codeloop/session/{provider}/{id}/messages",
            get(codeloop_messages),
        )
}

async fn health(State(s): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "db_path": s.db_path.display().to_string(),
    }))
}

#[derive(Debug, Deserialize)]
struct SubmitBody {
    kind: String,
    input: Value,
    #[serde(default)]
    callback_url: Option<String>,
}

async fn submit_task(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubmitBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let trace_parent = trace_ctx_from_headers(&headers);
    match toolkit_tasks::submit(
        &s.registry,
        &s.pool,
        &s.workspace,
        &body.kind,
        body.input,
        body.callback_url,
        trace_parent,
    ) {
        Ok(id) => Ok(Json(json!({ "task_id": id }))),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("{e:#}") })),
        )),
    }
}

async fn get_task(State(s): State<AppState>, Path(task_id): Path<String>) -> impl IntoResponse {
    match toolkit_tasks::status(&s.pool, &task_id) {
        Ok(Some(dto)) => (StatusCode::OK, Json(serde_json::to_value(dto).unwrap())),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "task not found"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e:#}")})),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    kind: Option<String>,
    state: Option<String>,
    limit: Option<i64>,
}

async fn list_tasks(State(s): State<AppState>, Query(q): Query<ListQuery>) -> impl IntoResponse {
    let filter = toolkit_tasks::TaskListFilter {
        kind: q.kind,
        state: q.state,
        limit: q.limit,
    };
    match toolkit_tasks::list_tasks(&s.pool, &filter) {
        Ok(v) => (StatusCode::OK, Json(serde_json::to_value(v).unwrap())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e:#}")})),
        ),
    }
}

// ---------------- codeloop 只读会话观测（Plan 2） ----------------

#[derive(Debug, Deserialize)]
struct SessionsQuery {
    #[serde(default = "default_sessions_limit")]
    limit: usize,
}

fn default_sessions_limit() -> usize {
    30
}

/// 列出本机 Codex / Claude 会话清单（供前端挑选配对）。
async fn codeloop_sessions(
    State(s): State<AppState>,
    Query(q): Query<SessionsQuery>,
) -> impl IntoResponse {
    match s.session_store.list(q.limit) {
        Ok(rows) => (StatusCode::OK, Json(serde_json::to_value(rows).unwrap())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e:#}")})),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct MessagesQuery {
    #[serde(default)]
    after: usize,
}

/// 增量取某会话消息（cursor = 已读行数）。
async fn codeloop_messages(
    State(s): State<AppState>,
    Path((provider, id)): Path<(String, String)>,
    Query(q): Query<MessagesQuery>,
) -> impl IntoResponse {
    let Some(provider) = agent_session::Provider::parse(&provider) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "provider 必须是 codex 或 claude"})),
        );
    };
    match s.session_store.messages(provider, &id, q.after) {
        Ok(page) => (StatusCode::OK, Json(serde_json::to_value(page).unwrap())),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("{e:#}")})),
        ),
    }
}
