//! `codeloop_io` 表读写：ASK_USER 挂起 → 用户回答的 DB 握手（见 RFC §10.3）。
//!
//! 任务体（`kind.rs`）insert 问题并轮询 answer_text；HTTP `/answer` 端点 update answer_text。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use toolkit_core::{now_iso8601, SqlitePool};

/// 插入一条待答问题，返回分配的 seq（同一 task 内递增）。
pub fn insert_question(
    pool: &SqlitePool,
    task_id: &str,
    asked_by: &str,
    question_json: &str,
) -> Result<i64> {
    let conn = pool.get()?;
    let next_seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM codeloop_io WHERE task_id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .context("compute next codeloop_io seq")?;
    conn.execute(
        "INSERT INTO codeloop_io(task_id, seq, asked_by, question_json, answer_text, created_at, answered_at)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, NULL)",
        params![task_id, next_seq, asked_by, question_json, now_iso8601()],
    )
    .context("insert codeloop_io question")?;
    Ok(next_seq)
}

/// 读某条问题的答案（NULL = 待答）。
pub fn read_answer(pool: &SqlitePool, task_id: &str, seq: i64) -> Result<Option<String>> {
    let conn = pool.get()?;
    let ans: Option<String> = conn
        .query_row(
            "SELECT answer_text FROM codeloop_io WHERE task_id = ?1 AND seq = ?2",
            params![task_id, seq],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(ans)
}

/// 写入用户答案（HTTP `/answer` 调用）。返回受影响行数（0 = 无此问题）。
pub fn write_answer(pool: &SqlitePool, task_id: &str, seq: i64, text: &str) -> Result<usize> {
    let conn = pool.get()?;
    let n = conn
        .execute(
            "UPDATE codeloop_io SET answer_text = ?1, answered_at = ?2 WHERE task_id = ?3 AND seq = ?4",
            params![text, now_iso8601(), task_id, seq],
        )
        .context("update codeloop_io answer")?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(dir: &std::path::Path) -> SqlitePool {
        let p = toolkit_core::open_pool(&dir.join("t.db")).unwrap();
        toolkit_core::migrate(&p).unwrap();
        p
    }

    #[test]
    fn insert_then_seq_increments_per_task() {
        let tmp = tempfile::tempdir().unwrap();
        let p = pool(tmp.path());
        let s1 = insert_question(&p, "t1", "codex", "{}").unwrap();
        let s2 = insert_question(&p, "t1", "claude", "{}").unwrap();
        let other = insert_question(&p, "t2", "codex", "{}").unwrap();
        assert_eq!(s1, 1);
        assert_eq!(s2, 2);
        assert_eq!(other, 1);
    }

    #[test]
    fn answer_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let p = pool(tmp.path());
        let seq = insert_question(&p, "t1", "codex", "{}").unwrap();
        assert_eq!(read_answer(&p, "t1", seq).unwrap(), None);
        let n = write_answer(&p, "t1", seq, "方案A").unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            read_answer(&p, "t1", seq).unwrap().as_deref(),
            Some("方案A")
        );
    }

    #[test]
    fn write_answer_missing_row_affects_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let p = pool(tmp.path());
        assert_eq!(write_answer(&p, "nope", 1, "x").unwrap(), 0);
    }
}
