use crate::kind::Registry;
use crate::runner::run_task;
use crate::store::{self, TaskRecord};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use toolkit_core::{new_task_id, SqlitePool};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatusDto {
    pub task_id: String,
    pub kind: String,
    pub state: String,
    pub progress: Value,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

impl From<TaskRecord> for TaskStatusDto {
    fn from(r: TaskRecord) -> Self {
        Self {
            task_id: r.task_id,
            kind: r.kind,
            state: r.state,
            progress: r.progress,
            output: r.output,
            error: r.error,
            created_at: r.created_at,
            started_at: r.started_at,
            finished_at: r.finished_at,
        }
    }
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct TaskListFilter {
    pub kind: Option<String>,
    pub state: Option<String>,
    pub limit: Option<i64>,
}

/// 提交一个任务，立刻 spawn 异步执行，返回 task_id。
pub fn submit(
    registry: &Registry,
    pool: &SqlitePool,
    data_dir: &Path,
    kind: &str,
    input: Value,
    callback_url: Option<String>,
) -> Result<String> {
    let erased = registry
        .get(kind)
        .ok_or_else(|| anyhow!("unknown kind: {kind}"))?;
    let task_id = new_task_id();
    store::insert_queued(pool, &task_id, kind, &input, callback_url.as_deref())?;
    let pool_clone = pool.clone();
    let id_clone = task_id.clone();
    let data_dir = data_dir.to_path_buf();
    tokio::spawn(run_task(
        Arc::clone(&erased),
        id_clone,
        input,
        pool_clone,
        data_dir,
    ));
    Ok(task_id)
}

pub fn status(pool: &SqlitePool, task_id: &str) -> Result<Option<TaskStatusDto>> {
    Ok(store::get(pool, task_id)?.map(Into::into))
}

pub fn list_tasks(pool: &SqlitePool, filter: &TaskListFilter) -> Result<Vec<TaskStatusDto>> {
    let limit = filter.limit.unwrap_or(100).clamp(1, 1000);
    let recs = store::list(pool, filter.kind.as_deref(), filter.state.as_deref(), limit)?;
    Ok(recs.into_iter().map(Into::into).collect())
}
