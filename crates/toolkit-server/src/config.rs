use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    /// Web 控制台静态目录。默认 `./web`；目录不存在则 fallback 到内嵌最小 HTML。
    pub web_dir: PathBuf,
}
