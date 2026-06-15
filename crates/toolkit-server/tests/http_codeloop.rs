//! Plan 2：codeloop 只读端点 + 嵌入式静态页。
//!
//! 用 agent-session crate 的 fixtures 作为会话存储 home（注入 AppState.session_store），
//! 不读真实用户 ~/.codex / ~/.claude。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use toolkit_server::{bind_ephemeral, bootstrap, build_router, AppState, Config};

fn fixtures_home() -> PathBuf {
    // crates/toolkit-server -> crates/agent-session/tests/fixtures
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../agent-session/tests/fixtures")
}

async fn start() -> (SocketAddr, TempDir) {
    let (listener, addr) = bind_ephemeral().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        bind: addr,
        workspace: dir.path().to_path_buf(),
        web_dir: PathBuf::from("/__nonexistent__"),
    };
    let mut state: AppState = bootstrap(&cfg).unwrap();
    // 注入 fixture 会话存储，确保测试确定性。
    state.session_store = Arc::new(agent_session::store::Store::with_home(fixtures_home()));
    let router = build_router(state, std::path::Path::new("/__nonexistent__"));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    (addr, dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codeloop_sessions_lists_both_providers() {
    let (addr, _dir) = start().await;
    let resp = reqwest::get(format!("http://{addr}/api/web/codeloop/sessions"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let rows: serde_json::Value = resp.json().await.unwrap();
    let arr = rows.as_array().unwrap();
    assert!(arr.iter().any(|r| r["provider"] == "codex"));
    assert!(arr.iter().any(|r| r["provider"] == "claude"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codeloop_messages_increment_by_cursor() {
    let (addr, _dir) = start().await;
    let id = "11111111-aaaa-bbbb-cccc-000000000001";
    let url = format!("http://{addr}/api/web/codeloop/session/codex/{id}/messages?after=0");
    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), 200);
    let page: serde_json::Value = resp.json().await.unwrap();
    let cursor = page["cursor"].as_u64().unwrap();
    assert!(!page["messages"].as_array().unwrap().is_empty());

    // 用游标增量再取一次：应为空。
    let url2 = format!("http://{addr}/api/web/codeloop/session/codex/{id}/messages?after={cursor}");
    let page2: serde_json::Value = reqwest::get(&url2).await.unwrap().json().await.unwrap();
    assert!(page2["messages"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codeloop_messages_rejects_bad_provider() {
    let (addr, _dir) = start().await;
    let resp = reqwest::get(format!(
        "http://{addr}/api/web/codeloop/session/bogus/xyz/messages"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codeloop_static_page_embedded() {
    let (addr, _dir) = start().await;
    let resp = reqwest::get(format!("http://{addr}/codeloop"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Codeloop"));

    let resp = reqwest::get(format!("http://{addr}/codeloop.js"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains("pollSide"));
}
