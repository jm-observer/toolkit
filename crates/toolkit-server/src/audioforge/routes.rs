//! AudioForge HTTP 路由（挂在 `/api/web/audio` 下，与 TTS 代理同前缀）：
//!   - `POST /forge`                      → 提交 `audio_forge` 长任务，返回 task_id
//!   - `GET  /forge/{package_id}/manifest.json` → 下载 manifest（english 拉取入口）
//!   - `GET  /forge/{package_id}/{file}`  → 下载某个音频文件（防路径穿越）
//!
//! 下载途径让 english `package.import` 能凭 manifest_url 拉取产物与音频，全程零人工传文件。

use crate::audioforge::manifest::ForgePaths;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/forge", post(forge_submit))
        .route("/forge/{package_id}/{file}", get(download))
}

/// `POST /api/web/audio/forge`：提交 audio_forge 任务（透传 traceparent）。
async fn forge_submit(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let trace_parent = custom_utils::trace::extract_traceparent(|name| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string())
    });
    let task_id = toolkit_tasks::submit(
        &s.registry,
        &s.pool,
        &s.workspace,
        "audio_forge",
        body,
        None,
        trace_parent,
    )
    .map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("{e:#}") })),
        )
    })?;
    Ok(Json(json!({ "task_id": task_id, "kind": "audio_forge" })))
}

/// `GET /api/web/audio/forge/{package_id}/{file}`：下载 manifest 或某个 wav。
///
/// 防路径穿越：`package_id` 与 `file` 都必须是单段「安全」名（无分隔符 / `..` / 绝对盘符），
/// 拼出的最终路径还要校验仍在 `<workspace>/audioforge/` 之内。
async fn download(
    State(s): State<AppState>,
    AxumPath((package_id, file)): AxumPath<(String, String)>,
) -> Response {
    if !is_safe_segment(&package_id) || !is_safe_segment(&file) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "非法路径段" })),
        )
            .into_response();
    }

    let paths = ForgePaths::new(&s.workspace);
    let pkg_dir = paths.package_dir(&package_id);
    let target = pkg_dir.join(&file);

    // 二次防御：规范化后必须仍落在 audioforge root 下。
    match (target.canonicalize(), paths.root.canonicalize()) {
        (Ok(t), Ok(root)) if t.starts_with(&root) => {}
        // 文件不存在时 canonicalize 失败 → 404（而非泄露路径细节）。
        _ => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "未找到" }))).into_response();
        }
    }

    match std::fs::read(&target) {
        Ok(bytes) => {
            let ct = if file.ends_with(".json") {
                "application/json; charset=utf-8"
            } else if file.ends_with(".wav") {
                "audio/wav"
            } else {
                "application/octet-stream"
            };
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, ct)],
                Bytes::from(bytes),
            )
                .into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, Json(json!({ "error": "未找到" }))).into_response(),
    }
}

/// 单段路径是否安全：非空、无路径分隔符、无 `..`、无盘符冒号。
fn is_safe_segment(seg: &str) -> bool {
    !seg.is_empty()
        && !seg.contains('/')
        && !seg.contains('\\')
        && !seg.contains("..")
        && !seg.contains(':')
        && seg != "."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_segment_rules() {
        assert!(is_safe_segment("001.wav"));
        assert!(is_safe_segment("manifest.json"));
        assert!(is_safe_segment("ab12cd34"));
        assert!(!is_safe_segment(""));
        assert!(!is_safe_segment(".."));
        assert!(!is_safe_segment("../etc"));
        assert!(!is_safe_segment("a/b"));
        assert!(!is_safe_segment("a\\b"));
        assert!(!is_safe_segment("C:"));
        assert!(!is_safe_segment("."));
    }
}
