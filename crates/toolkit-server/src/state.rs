use std::path::PathBuf;
use std::sync::Arc;
use toolkit_core::SqlitePool;
use toolkit_tasks::Registry;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub registry: Arc<Registry>,
    pub db_path: PathBuf,
    pub data_dir: PathBuf,
}
