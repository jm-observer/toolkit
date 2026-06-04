mod common;
use common::TestServer;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_health_ok() {
    let s = TestServer::start().await;
    let resp = reqwest::get(s.url("/api/web/health")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["status"], "ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_health_ok() {
    let s = TestServer::start().await;
    let resp = reqwest::get(s.url("/api/agent/health")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["namespace"], "agent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dashboard_serves_html() {
    let s = TestServer::start().await;
    let resp = reqwest::get(s.url("/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("toolkit-server"));
}
