use anyhow::{Context, Result};
use clap::Parser;
use toolkit_server::{run, Config};

#[derive(Parser, Debug)]
#[command(name = "toolkit-server", version)]
struct Cli {
    /// 监听地址，例如 0.0.0.0:8788
    #[arg(long, env = "TOOLKIT_BIND", default_value = "0.0.0.0:8788")]
    bind: String,
    /// 数据目录（SQLite 文件 + 中间产物）
    #[arg(long, env = "TOOLKIT_DATA_DIR", default_value = "./data")]
    data_dir: std::path::PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let bind: std::net::SocketAddr = cli.bind.parse().context("parse bind")?;
    run(Config {
        bind,
        data_dir: cli.data_dir,
    })
    .await
}
