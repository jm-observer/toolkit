use crate::db::SqlitePool;
use crate::schema::{DDL_V1, SCHEMA_VERSION};
use anyhow::{Context, Result};
use rusqlite::params;

/// 启动时调用一次，幂等。
pub fn migrate(pool: &SqlitePool) -> Result<()> {
    let conn = pool.get().context("acquire connection")?;
    conn.execute_batch(DDL_V1).context("apply v1 ddl")?;
    let current: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .ok();
    if current.is_none() {
        conn.execute(
            "INSERT INTO meta(key, value) VALUES('schema_version', ?1)",
            params![SCHEMA_VERSION.to_string()],
        )?;
    }
    Ok(())
}

/// 读 schema_version，主要给测试用。
pub fn schema_version(pool: &SqlitePool) -> Result<i64> {
    let conn = pool.get()?;
    let v: String = conn.query_row(
        "SELECT value FROM meta WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    )?;
    Ok(v.parse()?)
}
