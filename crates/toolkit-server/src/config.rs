use std::net::SocketAddr;
use std::path::PathBuf;

/// 进程启动配置。`workspace` 是所有持久状态（SQLite、cookie、tasks、knowledge、web）
/// 的统一根目录；默认 `$TOOLKIT_WORKSPACE` → `$HOME/.config/toolkit-server`，
/// 与 `LinuxService` 安装时 `{workspace}` 模板对齐。
#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub workspace: PathBuf,
    /// Web 控制台静态目录。默认 `<workspace>/web`；目录不存在则 fallback 到内嵌最小 HTML。
    pub web_dir: PathBuf,
}
