use agent_session::store::Store;
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
    /// codeloop 会话存储观测：只读解析本机 `~/.codex` / `~/.claude`（不在 workspace 下）。
    pub session_store: Arc<Store>,
}
