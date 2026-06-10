use crate::kind::Registry;
use crate::runner::run_task;
use crate::store::{self, TaskRecord};
use anyhow::{anyhow, Result};
use custom_utils::trace::{self, SpanScope, TraceContext};
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
///
/// `trace_parent`：上游传入的 trace 上下文（如 HTTP `traceparent` 解析所得的远端
/// 当前 span）。`Some` 时本任务作为其子 span 接入同一条 trace；`None` 时自起一条
/// 根 trace。trace 未初始化（未设 `TRACE_HUB_ENDPOINT`）时整段打点为 no-op。
pub fn submit(
    registry: &Registry,
    pool: &SqlitePool,
    data_dir: &Path,
    kind: &str,
    input: Value,
    callback_url: Option<String>,
    trace_parent: Option<TraceContext>,
) -> Result<String> {
    let erased = registry
        .get(kind)
        .ok_or_else(|| anyhow!("unknown kind: {kind}"))?;
    let task_id = new_task_id();
    store::insert_queued(pool, &task_id, kind, &input, callback_url.as_deref())?;

    // anchor span（两阶段第一阶段）：任务一进队列就落「正在排队 / 输入摘要」，
    // 即使后续运行很久或进程崩溃，trace-hub 也立刻能看到这次提交。
    let scope = trace::enabled().then(|| {
        let ctx = match &trace_parent {
            Some(parent) => parent.child(),
            None => TraceContext::root(),
        };
        let scope = SpanScope::new(ctx, "task")
            .with_flow_name(kind.to_string())
            .with_summary(serde_json::json!({
                "task_id": task_id,
                "kind": kind,
            }))
            .with_request_body(summarize_input(&input));
        scope.emit_start();
        scope
    });

    let pool_clone = pool.clone();
    let id_clone = task_id.clone();
    let data_dir = data_dir.to_path_buf();
    tokio::spawn(run_task(
        Arc::clone(&erased),
        id_clone,
        input,
        pool_clone,
        data_dir,
        scope,
    ));
    Ok(task_id)
}

/// 输入摘要：截断到合理长度，避免把超大 input 整体塞进 trace body。
/// （trace-hub 服务端也会按 body_limit 再截，这里先做一道便宜的预截断。）
fn summarize_input(input: &Value) -> String {
    const MAX: usize = 4096;
    let s = input.to_string();
    if s.len() > MAX {
        // 退到 <= MAX 的最近 char 边界，避免切断多字节字符 panic。
        let mut end = MAX;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…(truncated {} bytes)", &s[..end], s.len() - end)
    } else {
        s
    }
}

pub fn status(pool: &SqlitePool, task_id: &str) -> Result<Option<TaskStatusDto>> {
    Ok(store::get(pool, task_id)?.map(Into::into))
}

pub fn list_tasks(pool: &SqlitePool, filter: &TaskListFilter) -> Result<Vec<TaskStatusDto>> {
    let limit = filter.limit.unwrap_or(100).clamp(1, 1000);
    let recs = store::list(pool, filter.kind.as_deref(), filter.state.as_deref(), limit)?;
    Ok(recs.into_iter().map(Into::into).collect())
}
