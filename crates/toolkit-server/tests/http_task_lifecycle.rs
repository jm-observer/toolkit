mod common;
use common::TestServer;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_echo_succeeds() {
    let s = TestServer::start().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(s.url("/api/web/tasks"))
        .json(&json!({
            "kind": "echo",
            "input": {"message": "hi", "delay_ms": 100}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    let task_id = v["task_id"].as_str().unwrap().to_string();
    assert!(task_id.starts_with("tk_"));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let r = client
            .get(s.url(&format!("/api/web/tasks/{task_id}")))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let v: serde_json::Value = r.json().await.unwrap();
        let state = v["state"].as_str().unwrap();
        if state != "queued" && state != "running" {
            assert_eq!(state, "succeeded", "got {state}, err={:?}", v["error"]);
            assert_eq!(v["output"]["message"], "hi");
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("timeout, last state={state}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_kind_400() {
    let s = TestServer::start().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(s.url("/api/web/tasks"))
        .json(&json!({"kind": "no_such", "input": {}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert!(v["error"].as_str().unwrap().contains("unknown kind"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nonexistent_task_404() {
    let s = TestServer::start().await;
    let resp = reqwest::get(s.url("/api/web/tasks/tk_nope")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_tasks_returns_array() {
    let s = TestServer::start().await;
    let client = reqwest::Client::new();
    // 先提交一个
    client
        .post(s.url("/api/web/tasks"))
        .json(&json!({"kind":"echo","input":{"message":"x","delay_ms":10}}))
        .send()
        .await
        .unwrap();
    let resp = client.get(s.url("/api/web/tasks")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert!(v.is_array());
    assert!(!v.as_array().unwrap().is_empty());
}
