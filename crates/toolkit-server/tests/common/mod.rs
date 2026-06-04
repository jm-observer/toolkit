use std::net::SocketAddr;
use tempfile::TempDir;
use toolkit_server::{bind_ephemeral, bootstrap, serve_with_listener, Config};

#[allow(dead_code)] // 字段保留在结构体里防止临时目录提前 drop
pub struct TestServer {
    pub addr: SocketAddr,
    pub dir: TempDir,
    pub handle: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl TestServer {
    pub async fn start() -> Self {
        let (listener, addr) = bind_ephemeral().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            bind: addr,
            data_dir: dir.path().to_path_buf(),
        };
        let state = bootstrap(&cfg).unwrap();
        let handle = tokio::spawn(serve_with_listener(listener, state));
        // 等 server ready
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        TestServer { addr, dir, handle }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}
