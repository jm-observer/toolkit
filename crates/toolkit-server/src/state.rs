use std::path::PathBuf;
use std::sync::Arc;
use toolkit_core::SqlitePool;
use toolkit_tasks::Registry;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub registry: Arc<Registry>,
    pub db_path: PathBuf,
    /// workspace 根（toolkit.db / douyin/cookies.json / downloads/ / knowledge/ 等都在此下）。
    pub workspace: PathBuf,
}
