use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/tasks", get(list_tasks).post(submit_task))
        .route("/tasks/{task_id}", get(get_task))
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
    Json(body): Json<SubmitBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match toolkit_tasks::submit(
        &s.registry,
        &s.pool,
        &s.workspace,
        &body.kind,
        body.input,
        body.callback_url,
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
