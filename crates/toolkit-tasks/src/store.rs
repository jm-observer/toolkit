//! tasks 表读写。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use toolkit_core::{now_iso8601, SqlitePool};

pub fn insert_queued(
    pool: &SqlitePool,
    task_id: &str,
    kind: &str,
    input: &Value,
    callback_url: Option<&str>,
) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO tasks(task_id, kind, state, input, progress, created_at, callback_url)
         VALUES (?1, ?2, 'queued', ?3, '{}', ?4, ?5)",
        params![
            task_id,
            kind,
            serde_json::to_string(input)?,
            now_iso8601(),
            callback_url,
        ],
    )
    .context("insert task")?;
    Ok(())
}

pub fn mark_running(pool: &SqlitePool, task_id: &str) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "UPDATE tasks SET state='running', started_at=?1 WHERE task_id=?2",
        params![now_iso8601(), task_id],
    )?;
    Ok(())
}

pub fn mark_succeeded(pool: &SqlitePool, task_id: &str, output: &Value) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "UPDATE tasks SET state='succeeded', output=?1, finished_at=?2 WHERE task_id=?3",
        params![serde_json::to_string(output)?, now_iso8601(), task_id],
    )?;
    Ok(())
}

pub fn mark_failed(pool: &SqlitePool, task_id: &str, error: &str) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "UPDATE tasks SET state='failed', error=?1, finished_at=?2 WHERE task_id=?3",
        params![error, now_iso8601(), task_id],
    )?;
    Ok(())
}

pub fn update_progress(pool: &SqlitePool, task_id: &str, progress: &Value) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "UPDATE tasks SET progress=?1 WHERE task_id=?2",
        params![serde_json::to_string(progress)?, task_id],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub task_id: String,
    pub kind: String,
    pub state: String,
    pub input: Value,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub progress: Value,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

pub fn get(pool: &SqlitePool, task_id: &str) -> Result<Option<TaskRecord>> {
    let conn = pool.get()?;
    let rec = conn
        .query_row(
            "SELECT task_id, kind, state, input, output, error, progress, created_at, started_at, finished_at
             FROM tasks WHERE task_id=?1",
            params![task_id],
            row_to_record,
        )
        .optional()?;
    Ok(rec)
}

pub fn list(
    pool: &SqlitePool,
    kind: Option<&str>,
    state: Option<&str>,
    limit: i64,
) -> Result<Vec<TaskRecord>> {
    let conn = pool.get()?;
    let (sql, params_dyn): (String, Vec<Box<dyn rusqlite::ToSql>>) = match (kind, state) {
        (Some(k), Some(s)) => (
            "SELECT task_id, kind, state, input, output, error, progress, created_at, started_at, finished_at
             FROM tasks WHERE kind=?1 AND state=?2 ORDER BY created_at DESC LIMIT ?3".into(),
            vec![Box::new(k.to_string()), Box::new(s.to_string()), Box::new(limit)],
        ),
        (Some(k), None) => (
            "SELECT task_id, kind, state, input, output, error, progress, created_at, started_at, finished_at
             FROM tasks WHERE kind=?1 ORDER BY created_at DESC LIMIT ?2".into(),
            vec![Box::new(k.to_string()), Box::new(limit)],
        ),
        (None, Some(s)) => (
            "SELECT task_id, kind, state, input, output, error, progress, created_at, started_at, finished_at
             FROM tasks WHERE state=?1 ORDER BY created_at DESC LIMIT ?2".into(),
            vec![Box::new(s.to_string()), Box::new(limit)],
        ),
        (None, None) => (
            "SELECT task_id, kind, state, input, output, error, progress, created_at, started_at, finished_at
             FROM tasks ORDER BY created_at DESC LIMIT ?1".into(),
            vec![Box::new(limit)],
        ),
    };
    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::ToSql> = params_dyn.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(refs.as_slice(), row_to_record)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecord> {
    let input_s: String = r.get(3)?;
    let output_s: Option<String> = r.get(4)?;
    let progress_s: String = r.get(6)?;
    Ok(TaskRecord {
        task_id: r.get(0)?,
        kind: r.get(1)?,
        state: r.get(2)?,
        input: serde_json::from_str(&input_s).unwrap_or(Value::Null),
        output: output_s.and_then(|s| serde_json::from_str(&s).ok()),
        error: r.get(5)?,
        progress: serde_json::from_str(&progress_s).unwrap_or(Value::Null),
        created_at: r.get(7)?,
        started_at: r.get(8)?,
        finished_at: r.get(9)?,
    })
}
