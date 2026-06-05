use anyhow::{Context, Result};
use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;

/// 共享连接池类型。
pub type SqlitePool = r2d2::Pool<SqliteConnectionManager>;

/// 打开（或创建）SQLite 文件并返回连接池。
///
/// 父目录必须已存在；本函数不创建目录。
pub fn open_pool(path: &Path) -> Result<SqlitePool> {
    let manager = SqliteConnectionManager::file(path).with_init(|c| {
        // busy_timeout 先设，让后续 PRAGMA 在并发预热时能内部等而非立刻 "database is locked"。
        c.execute_batch(
            "PRAGMA busy_timeout=5000;\n\
             PRAGMA journal_mode=WAL;\n\
             PRAGMA synchronous=NORMAL;\n\
             PRAGMA foreign_keys=OFF;",
        )
    });
    let pool = r2d2::Pool::builder()
        .max_size(8)
        .build(manager)
        .with_context(|| format!("open sqlite pool: {}", path.display()))?;
    Ok(pool)
}
