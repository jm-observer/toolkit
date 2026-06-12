pub mod repository;
pub mod schema;

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::Local;
use rusqlite::Connection;

/// Cloneable handle to the speech SQLite database.
#[derive(Clone)]
pub struct SpeechDatabase {
    conn: Arc<Mutex<Connection>>,
}

impl SpeechDatabase {
    pub async fn init(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("failed to create database directory: {}", parent.display())
            })?;
        }

        let db_path = db_path.to_path_buf();
        let conn = tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)
                .with_context(|| format!("failed to open speech db at {}", db_path.display()))?;
            schema::run_migrations(&conn)?;
            Ok::<Connection, anyhow::Error>(conn)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn upsert_setting(&self, key: String, value: String) -> Result<()> {
        let now = now_str();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_| anyhow::anyhow!("db mutex poisoned"))?;
            repository::upsert_setting(&guard, &key, &value, &now)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;
        Ok(())
    }

    pub async fn get_setting(&self, key: String) -> Result<Option<String>> {
        let conn = Arc::clone(&self.conn);
        let result = tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_| anyhow::anyhow!("db mutex poisoned"))?;
            repository::get_setting(&guard, &key)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;
        Ok(result)
    }
}

fn now_str() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}
