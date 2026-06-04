use crate::kind::{ErasedKind, TaskCtx};
use crate::store;
use anyhow::Result;
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
pub(crate) async fn run_task(
    erased: Arc<dyn ErasedKind>,
    task_id: String,
    input: serde_json::Value,
    pool: SqlitePool,
    data_dir: PathBuf,
) {
    if let Err(e) = store::mark_running(&pool, &task_id) {
        log::error!("mark_running({task_id}) failed: {e:#}");
        return;
    }

    let ctx = TaskCtx {
        task_id: task_id.clone(),
        pool: pool.clone(),
        data_dir,
    };

    // 把任务体放进一个独立 tokio task 跑，便于捕获 panic。
    let handle = tokio::spawn(async move { erased.run_json(input, ctx).await });

    match handle.await {
        Ok(Ok(output)) => {
            if let Err(e) = store::mark_succeeded(&pool, &task_id, &output) {
                log::error!("mark_succeeded({task_id}) failed: {e:#}");
            }
        }
        Ok(Err(e)) => {
            let msg = format!("{e:#}");
            if let Err(e2) = store::mark_failed(&pool, &task_id, &msg) {
                log::error!("mark_failed({task_id}) failed: {e2:#}");
            }
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
        }
    }
}
