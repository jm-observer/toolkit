//! axum HTTP 服务：`/healthz` / `/v1/search` / `/v1/ingest`。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ingest::ingest_douyin_knowledge;
use crate::service::KnowledgeRagService;
use crate::types::SearchQuery;
use crate::DOUYIN_NAMESPACE;

#[derive(Clone)]
struct AppState {
    service: Arc<KnowledgeRagService>,
    workspace_root: PathBuf,
    default_namespace: String,
}

#[derive(Deserialize)]
struct SearchReq {
    namespace: Option<String>,
    query: String,
    top_k: Option<usize>,
}

#[derive(Deserialize)]
struct IngestReq {
    namespace: Option<String>,
}

/// 起 HTTP 服务，阻塞直到进程退出。
pub async fn run(service: KnowledgeRagService, workspace_root: PathBuf, bind: &str) -> Result<()> {
    let state = AppState {
        service: Arc::new(service),
        workspace_root,
        default_namespace: DOUYIN_NAMESPACE.to_string(),
    };
    let app = Router::new()
        .route("/healthz", get(healthz))
        // GET（query 串）便于 mcp-server http 工具与 curl；POST（JSON body）便于程序化调用。
        .route("/v1/search", get(search_get).post(search_post))
        .route("/v1/ingest", post(ingest))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    log::info!("rag serve listening on {bind}");
    axum::serve(listener, app).await.context("axum serve")?;
    Ok(())
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn search_post(State(st): State<AppState>, Json(req): Json<SearchReq>) -> Json<Value> {
    do_search(&st, req.namespace, req.query, req.top_k).await
}

async fn search_get(State(st): State<AppState>, Query(req): Query<SearchReq>) -> Json<Value> {
    do_search(&st, req.namespace, req.query, req.top_k).await
}

async fn do_search(
    st: &AppState,
    namespace: Option<String>,
    query: String,
    top_k: Option<usize>,
) -> Json<Value> {
    let namespace = namespace.unwrap_or_else(|| st.default_namespace.clone());
    let query = SearchQuery {
        namespace,
        query,
        top_k: top_k.unwrap_or(5),
    };
    match st.service.search(query).await {
        Ok(hits) => Json(json!({ "hits": hits })),
        Err(e) => Json(json!({ "error": e.to_string(), "error_kind": "search" })),
    }
}

async fn ingest(State(st): State<AppState>, Json(req): Json<IngestReq>) -> Json<Value> {
    let namespace = req
        .namespace
        .unwrap_or_else(|| st.default_namespace.clone());
    match ingest_douyin_knowledge(&st.service, &st.workspace_root, &namespace).await {
        Ok(stats) => Json(json!({
            "ingested": stats.ingested,
            "skipped": stats.skipped,
            "failed": stats.failed,
        })),
        Err(e) => Json(json!({ "error": e.to_string(), "error_kind": "ingest" })),
    }
}
