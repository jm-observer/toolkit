//! append-only 事件日志（设计 §Event Log）。
//!
//! 每任务一条 `<task_id>.events.jsonl`，逐行 JSON 记录生命周期事件
//! （job.created / job.started / item.* / job.<终态> / callback.*）。用途：
//! Web 时间线、CLI `events` 查询、webhook 失败时的最终可信记录。
//!
//! 写入 best-effort——记日志失败只 warn，绝不阻断任务本身。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Event {
    pub ts: String,
    pub task_id: String,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn events_path(task_dir: &Path, task_id: &str) -> PathBuf {
    task_dir.join(format!("{task_id}.events.jsonl"))
}

/// 追加一条事件（best-effort）。失败只 warn，不返回错误、不阻断调用方。
pub fn append(task_dir: &Path, task_id: &str, event: &str, detail: Option<Value>) {
    let ev = Event {
        ts: now(),
        task_id: task_id.to_string(),
        event: event.to_string(),
        detail,
    };
    if let Err(e) = append_inner(task_dir, &ev) {
        log::warn!("[events] append failed task_id={task_id} event={event}: {e}");
    }
}

fn append_inner(task_dir: &Path, ev: &Event) -> Result<()> {
    let line = format!("{}\n", serde_json::to_string(ev)?);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_path(task_dir, &ev.task_id))?;
    f.write_all(line.as_bytes())?;
    Ok(())
}

/// 读某任务的全部事件（按写入顺序）。坏行跳过。
pub fn read_all(task_dir: &Path, task_id: &str) -> Result<Vec<Event>> {
    let p = events_path(task_dir, task_id);
    if !p.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&p)?;
    Ok(raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Event>(l).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        let id = C.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("douyin-ev-test-{}-{}", std::process::id(), id));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn append_then_read_in_order() {
        let dir = tempdir();
        append(&dir, "dyproc1", "job.created", Some(json!({"total": 2})));
        append(&dir, "dyproc1", "job.started", None);
        append(&dir, "dyproc1", "job.succeeded", None);
        let evs = read_all(&dir, "dyproc1").unwrap();
        assert_eq!(evs.len(), 3);
        assert_eq!(evs[0].event, "job.created");
        assert_eq!(evs[0].detail, Some(json!({"total": 2})));
        assert_eq!(evs[2].event, "job.succeeded");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_missing_returns_empty() {
        let dir = tempdir();
        assert!(read_all(&dir, "nope").unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
