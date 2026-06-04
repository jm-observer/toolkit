use crate::state::AppState;
use axum::extract::State;
use axum::response::Json;
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new().route("/health", get(health))
}

async fn health(State(s): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "db_path": s.db_path.display().to_string(),
        "namespace": "agent",
    }))
}
