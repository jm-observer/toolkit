//! sqlite-vec 后端的 [`VectorStore`] 实现。
//!
//! 设计点：
//!
//! - 进程内一次性 `sqlite3_auto_extension` 注册 vec0，使后续所有 `Connection::open` 自动加载
//! - `Connection` 由 `Arc<Mutex<...>>` 串行化访问；trait 方法体走 `tokio::task::spawn_blocking`
//! - `chunks_vec.rowid` 与 `chunks.id` 一一对应，事务保证一致
//! - 启动期校验已有 `chunks_vec` 维度与 config 一致；不一致即报错引导用户删 rag.db 重 ingest
//! - 距离度量：vec0 默认 L2，本实现用余弦距离；score = `1 - cosine_distance` ∈ \[0,1\]，越大越相似
//! - 跨 namespace 检索 oversample 4 倍 + 应用层 JOIN 过滤

use std::path::Path;
use std::sync::{Arc, Mutex, Once};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::config::RagStoreConfig;
use crate::store::{StoredChunk, VectorStore};
use crate::types::SearchHit;

static VEC_EXT_INIT: Once = Once::new();

fn ensure_vec_extension_registered() {
    VEC_EXT_INIT.call_once(|| {
        // SAFETY: sqlite_vec FFI symbol；按 sqlite-vec crate 0.1.x README 推荐路径一次性注册。
        // 后续所有 `Connection::open*` 自动加载 vec0 扩展。重复调用无害但 `Once` 已防。
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                unsafe extern "C" fn(),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *const std::os::raw::c_char,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> std::os::raw::c_int,
            >(sqlite_vec::sqlite3_vec_init)));
        }
    });
}

#[derive(Debug)]
pub struct SqliteVecStore {
    conn: Arc<Mutex<Connection>>,
    dim: usize,
}

impl SqliteVecStore {
    pub async fn open(
        workspace_root: &Path,
        cfg: &RagStoreConfig,
        expected_dim: usize,
    ) -> Result<Self> {
        if expected_dim == 0 {
            return Err(anyhow!("sqlite-vec store: expected_dim must be > 0"));
        }
        ensure_vec_extension_registered();
        let db_path = workspace_root.join(&cfg.db_path);
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create db parent dir {}", parent.display()))?;
        }
        let db_path_str = db_path.to_string_lossy().to_string();
        tokio::task::spawn_blocking(move || -> Result<Self> {
            let conn = Connection::open(&db_path_str)
                .with_context(|| format!("open sqlite db {db_path_str}"))?;
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;\nPRAGMA foreign_keys=ON;\nPRAGMA synchronous=NORMAL;",
            )
            .context("apply pragmas")?;
            init_schema(&conn, expected_dim)?;
            Ok(SqliteVecStore {
                conn: Arc::new(Mutex::new(conn)),
                dim: expected_dim,
            })
        })
        .await
        .context("spawn_blocking open store")?
    }

    pub fn dim(&self) -> usize {
        self.dim
    }
}

fn init_schema(conn: &Connection, expected_dim: usize) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chunks (\n\
             id INTEGER PRIMARY KEY AUTOINCREMENT,\n\
             namespace TEXT NOT NULL,\n\
             external_id TEXT NOT NULL,\n\
             chunk_index INTEGER NOT NULL,\n\
             text TEXT NOT NULL,\n\
             metadata TEXT NOT NULL,\n\
             UNIQUE(namespace, external_id, chunk_index)\n\
         );\n\
         CREATE INDEX IF NOT EXISTS idx_chunks_ns_ext ON chunks(namespace, external_id);",
    )
    .context("init chunks schema")?;

    // 检查 chunks_vec 是否已存在；若存在则校验维度
    let existing: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunks_vec'",
            [],
            |r| r.get(0),
        )
        .optional()
        .context("query existing chunks_vec schema")?;

    if let Some(sql) = existing {
        // CREATE VIRTUAL TABLE 语句的 sql 含 "float[N]" 子串；提取并校验
        let dim_from_sql = extract_dim_from_create_sql(&sql)
            .ok_or_else(|| anyhow!("cannot parse dim from existing chunks_vec sql: {sql}"))?;
        if dim_from_sql != expected_dim {
            return Err(anyhow!(
                "rag.db dim mismatch: existing chunks_vec has dim={dim_from_sql}, config dim={expected_dim}; \
                 删除 rag.db 后重新 ingest"
            ));
        }
    } else {
        let create_sql =
            format!("CREATE VIRTUAL TABLE chunks_vec USING vec0(embedding float[{expected_dim}])");
        conn.execute(&create_sql, [])
            .with_context(|| format!("create chunks_vec dim={expected_dim}"))?;
    }
    Ok(())
}

fn extract_dim_from_create_sql(sql: &str) -> Option<usize> {
    let lb = sql.find('[')?;
    let rb = sql[lb..].find(']')?;
    sql[lb + 1..lb + rb].trim().parse().ok()
}

/// vec0 接受 JSON 文本格式或 blob；本实现用 JSON 文本，可读性好、避免 endian 假设。
fn vector_to_json(vec: &[f32]) -> String {
    let mut s = String::from("[");
    for (i, v) in vec.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&v.to_string());
    }
    s.push(']');
    s
}

#[async_trait]
impl VectorStore for SqliteVecStore {
    async fn upsert(
        &self,
        namespace: &str,
        external_id: &str,
        chunks: Vec<StoredChunk>,
    ) -> Result<()> {
        let dim = self.dim;
        // 入口预校验：早失败，事务都不开
        for (i, c) in chunks.iter().enumerate() {
            if c.vector.len() != dim {
                return Err(anyhow!(
                    "upsert dim mismatch at chunk {i}: expected={dim} actual={}",
                    c.vector.len()
                ));
            }
        }
        let conn = Arc::clone(&self.conn);
        let ns = namespace.to_string();
        let ext = external_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut guard = conn.lock().map_err(|_| anyhow!("conn mutex poisoned"))?;
            let tx = guard.transaction().context("begin tx")?;
            // 1) 查旧 ids
            let mut old_ids: Vec<i64> = Vec::new();
            {
                let mut stmt = tx
                    .prepare("SELECT id FROM chunks WHERE namespace = ?1 AND external_id = ?2")
                    .context("prepare select old ids")?;
                let rows = stmt
                    .query_map(params![ns, ext], |r| r.get::<_, i64>(0))
                    .context("query old ids")?;
                for r in rows {
                    old_ids.push(r?);
                }
            }
            // 2) 删 chunks_vec 中对应 rowids
            for id in &old_ids {
                tx.execute("DELETE FROM chunks_vec WHERE rowid = ?1", params![id])
                    .context("delete old vec row")?;
            }
            // 3) 删 chunks
            tx.execute(
                "DELETE FROM chunks WHERE namespace = ?1 AND external_id = ?2",
                params![ns, ext],
            )
            .context("delete old chunks")?;
            // 4) 插新
            for chunk in &chunks {
                let meta_str = chunk.metadata.to_string();
                tx.execute(
                    "INSERT INTO chunks(namespace, external_id, chunk_index, text, metadata) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![ns, ext, chunk.chunk_index as i64, chunk.text, meta_str],
                )
                .context("insert chunk row")?;
                let new_id = tx.last_insert_rowid();
                let vec_json = vector_to_json(&chunk.vector);
                tx.execute(
                    "INSERT INTO chunks_vec(rowid, embedding) VALUES (?1, ?2)",
                    params![new_id, vec_json],
                )
                .context("insert vec row")?;
            }
            tx.commit().context("commit tx")?;
            Ok(())
        })
        .await
        .context("spawn_blocking upsert")?
    }

    async fn search(
        &self,
        namespace: &str,
        query_vec: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchHit>> {
        if top_k == 0 {
            return Ok(Vec::new());
        }
        if query_vec.is_empty() {
            return Err(anyhow!("search: query_vec empty"));
        }
        if query_vec.len() != self.dim {
            return Err(anyhow!(
                "search dim mismatch: expected={} actual={}",
                self.dim,
                query_vec.len()
            ));
        }
        let conn = Arc::clone(&self.conn);
        let ns = namespace.to_string();
        let q_json = vector_to_json(query_vec);
        let oversample = (top_k * 4).max(top_k + 8);
        tokio::task::spawn_blocking(move || -> Result<Vec<SearchHit>> {
            let guard = conn.lock().map_err(|_| anyhow!("conn mutex poisoned"))?;
            // vec0 MATCH 内核取 top-k；这里 oversample 4 倍后应用层 namespace 过滤再 cut top_k。
            let sql =
                "SELECT v.rowid, v.distance, c.external_id, c.chunk_index, c.text, c.metadata \
                       FROM chunks_vec v \
                       JOIN chunks c ON c.id = v.rowid \
                       WHERE v.embedding MATCH ?1 AND k = ?2 AND c.namespace = ?3 \
                       ORDER BY v.distance";
            let mut stmt = guard.prepare(sql).context("prepare search stmt")?;
            let mut rows = stmt
                .query(params![q_json, oversample as i64, ns])
                .context("execute search")?;
            let mut hits = Vec::new();
            while let Some(row) = rows.next()? {
                let distance: f64 = row.get(1)?;
                let external_id: String = row.get(2)?;
                let chunk_index: i64 = row.get(3)?;
                let text: String = row.get(4)?;
                let meta_str: String = row.get(5)?;
                let metadata: Value = serde_json::from_str(&meta_str).unwrap_or(Value::Null);
                hits.push(SearchHit {
                    external_id,
                    chunk_index: chunk_index as usize,
                    text,
                    score: (1.0 - distance) as f32,
                    metadata,
                });
                if hits.len() >= top_k {
                    break;
                }
            }
            Ok(hits)
        })
        .await
        .context("spawn_blocking search")?
    }

    async fn delete(&self, namespace: &str, external_id: &str) -> Result<()> {
        let conn = Arc::clone(&self.conn);
        let ns = namespace.to_string();
        let ext = external_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut guard = conn.lock().map_err(|_| anyhow!("conn mutex poisoned"))?;
            let tx = guard.transaction().context("begin tx")?;
            let mut old_ids: Vec<i64> = Vec::new();
            {
                let mut stmt = tx
                    .prepare("SELECT id FROM chunks WHERE namespace = ?1 AND external_id = ?2")
                    .context("prepare select ids")?;
                let rows = stmt
                    .query_map(params![ns, ext], |r| r.get::<_, i64>(0))
                    .context("query ids")?;
                for r in rows {
                    old_ids.push(r?);
                }
            }
            for id in &old_ids {
                tx.execute("DELETE FROM chunks_vec WHERE rowid = ?1", params![id])
                    .context("delete vec row")?;
            }
            tx.execute(
                "DELETE FROM chunks WHERE namespace = ?1 AND external_id = ?2",
                params![ns, ext],
            )
            .context("delete chunks")?;
            tx.commit().context("commit tx")?;
            Ok(())
        })
        .await
        .context("spawn_blocking delete")?
    }
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn extract_dim_from_create_sql_basic() {
        let sql = "CREATE VIRTUAL TABLE chunks_vec USING vec0(embedding float[768])";
        assert_eq!(extract_dim_from_create_sql(sql), Some(768));
    }

    #[test]
    fn extract_dim_from_create_sql_spaces() {
        let sql = "CREATE VIRTUAL TABLE chunks_vec USING vec0(embedding float[ 1024 ])";
        assert_eq!(extract_dim_from_create_sql(sql), Some(1024));
    }

    #[test]
    fn vector_to_json_basic() {
        assert_eq!(vector_to_json(&[1.0, 2.5, -3.0]), "[1,2.5,-3]");
    }
}
