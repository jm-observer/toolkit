//! 验证：当 web_dir 存在时，/ 由 ServeDir 托管；否则走内嵌 dashboard。

use std::net::SocketAddr;
use tempfile::TempDir;
use toolkit_server::{bind_ephemeral, bootstrap, build_router, AppState, Config};

async fn start_with_web(web_dir: std::path::PathBuf) -> (SocketAddr, TempDir, AppState) {
    let (listener, addr) = bind_ephemeral().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        bind: addr,
        data_dir: dir.path().to_path_buf(),
        web_dir: web_dir.clone(),
    };
    let state = bootstrap(&cfg).unwrap();
    let router = build_router(state.clone(), &web_dir);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    (addr, dir, state)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_dashboard_when_web_dir_missing() {
    let (addr, _dir, _state) = start_with_web("/__definitely_not_present__".into()).await;
    let resp = reqwest::get(format!("http://{addr}/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("toolkit-server"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serves_static_when_web_dir_exists() {
    let web = tempfile::tempdir().unwrap();
    std::fs::write(
        web.path().join("index.html"),
        "<!DOCTYPE html><title>static-ok</title>",
    )
    .unwrap();
    std::fs::write(web.path().join("app.js"), "/* hello */").unwrap();

    let (addr, _dir, _state) = start_with_web(web.path().to_path_buf()).await;

    let resp = reqwest::get(format!("http://{addr}/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("static-ok"), "got: {body}");

    let resp = reqwest::get(format!("http://{addr}/app.js")).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains("hello"));

    // API 路由优先于静态
    let resp = reqwest::get(format!("http://{addr}/api/web/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
