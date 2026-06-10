use crate::kind::{ErasedKind, TaskCtx};
use crate::store;
use anyhow::Result;
use custom_utils::trace::{SpanScope, SpanStatus};
use rusqlite::params;
use std::path::PathBuf;
use std::sync::Arc;
use toolkit_core::{now_iso8601, SqlitePool};

/// 进程启动时调用一次：把残留的 queued/running 任务标为 interrupted。
pub fn recover_interrupted(pool: &SqlitePool) -> Result<usize> {
    let conn = pool.get()?;
    let n = conn.execute(
        "UPDATE tasks SET state='interrupted', error='process restart', finished_at=?1
         WHERE state IN ('queued','running')",
        params![now_iso8601()],
    )?;
    Ok(n)
}

/// 在 tokio runtime 中跑一个 task：状态机 + panic 捕获 + 持久化。
///
/// `scope`：submit 时建好并已 `emit_start` 的 anchor span（两阶段第一阶段）。任务
/// 终态时在此 `emit_end`（同 span_id 覆盖，补 response/耗时/终态 status）。`None`
/// 表示追踪未启用——整段打点为 no-op。
pub(crate) async fn run_task(
    erased: Arc<dyn ErasedKind>,
    task_id: String,
    input: serde_json::Value,
    pool: SqlitePool,
    data_dir: PathBuf,
    scope: Option<SpanScope>,
) {
    if let Err(e) = store::mark_running(&pool, &task_id) {
        log::error!("mark_running({task_id}) failed: {e:#}");
        if let Some(s) = scope {
            s.emit_end(
                Some(format!("mark_running failed: {e:#}")),
                SpanStatus::Error("mark_running failed".into()),
                Some(serde_json::json!({ "state": "error" })),
            );
        }
        return;
    }

    let ctx = TaskCtx {
        task_id: task_id.clone(),
        pool: pool.clone(),
        data_dir,
    };

    // 把任务体放进一个独立 tokio task 跑，便于捕获 panic。
    let handle = tokio::spawn(async move { erased.run_json(input, ctx).await });

    // 收尾：写状态机 + emit 完成 span（成功/失败、耗时由 SpanScope 自动补 dur_ms）。
    let (response_body, status, final_state) = match handle.await {
        Ok(Ok(output)) => {
            if let Err(e) = store::mark_succeeded(&pool, &task_id, &output) {
                log::error!("mark_succeeded({task_id}) failed: {e:#}");
            }
            (Some(output.to_string()), SpanStatus::Ok, "succeeded")
        }
        Ok(Err(e)) => {
            let msg = format!("{e:#}");
            if let Err(e2) = store::mark_failed(&pool, &task_id, &msg) {
                log::error!("mark_failed({task_id}) failed: {e2:#}");
            }
            (Some(msg.clone()), SpanStatus::Error(msg), "failed")
        }
        Err(je) => {
            let msg = if je.is_panic() {
                format!("task panicked: {je}")
            } else {
                format!("task join error: {je}")
            };
            if let Err(e2) = store::mark_failed(&pool, &task_id, &msg) {
                log::error!("mark_failed({task_id}) failed: {e2:#}");
            }
            (Some(msg.clone()), SpanStatus::Error(msg), "failed")
        }
    };

    if let Some(s) = scope {
        s.emit_end(
            response_body,
            status,
            Some(serde_json::json!({ "state": final_state })),
        );
    }
}
