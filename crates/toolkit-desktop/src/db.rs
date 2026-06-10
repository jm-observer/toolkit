//! `<workspace>/state.db` — 上传历史 + 浏览器 session 表。
//!
//! 单连接 + Mutex 模型：桌面端低并发，省掉连接池。表迁移幂等。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

pub struct Db {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UploadRow {
    pub id: i64,
    pub ts: String,
    pub hash: String,
    pub fields_count: i64,
    pub success: bool,
    pub server_response: Option<String>,
    pub error: Option<String>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn =
            Connection::open(path).with_context(|| format!("open sqlite {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS uploads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                hash TEXT NOT NULL,
                fields_count INTEGER NOT NULL,
                success INTEGER NOT NULL,
                server_response TEXT,
                error TEXT
            );
            CREATE INDEX IF NOT EXISTS uploads_ts ON uploads(ts DESC);

            CREATE TABLE IF NOT EXISTS browser_sessions (
                session_id TEXT PRIMARY KEY,
                first_seen TEXT NOT NULL,
                last_seen  TEXT NOT NULL
            );
            "#,
        )
        .context("migrate state.db")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn record_upload(
        &self,
        ts: &str,
        hash: &str,
        fields_count: i64,
        success: bool,
        server_response: Option<&str>,
        error: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO uploads(ts, hash, fields_count, success, server_response, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                ts,
                hash,
                fields_count,
                success as i64,
                server_response,
                error
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn recent_uploads(&self, limit: i64) -> Result<Vec<UploadRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, ts, hash, fields_count, success, server_response, error
             FROM uploads ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], |row| {
                Ok(UploadRow {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    hash: row.get(2)?,
                    fields_count: row.get(3)?,
                    success: row.get::<_, i64>(4)? != 0,
                    server_response: row.get(5)?,
                    error: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn upsert_session(&self, session_id: &str, now: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO browser_sessions(session_id, first_seen, last_seen)
             VALUES (?1, ?2, ?2)
             ON CONFLICT(session_id) DO UPDATE SET last_seen = excluded.last_seen",
            params![session_id, now],
        )?;
        Ok(())
    }
}
