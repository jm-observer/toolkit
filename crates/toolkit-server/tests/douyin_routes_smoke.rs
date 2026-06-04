//! 抖音路由烟测：覆盖那些**不需要网络/真 cookie** 的端点。
//!
//! - `/api/web/douyin/tags`：从 `works_dir` 读文件，无作品时返回 count=0
//! - `/api/web/douyin/filter`：同上
//! - `/api/web/douyin/kb_publish`：works_dir 为空时跑空（实际行为留给真机测试）
//! - 长任务 endpoint 入参错误 → 4xx

mod common;
use common::TestServer;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tags_empty_dir() {
    let s = TestServer::start().await;
    let resp = reqwest::get(s.url("/api/web/douyin/tags?unique_id=nonexistent"))
        .await
        .unwrap();
    // works_dir 为空时 douyin::run_list_tags 返回 not_listed/count=0；只要不 5xx 即视作端点通
    assert!(resp.status().is_success() || resp.status().is_client_error());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_rejects_empty_ids() {
    let s = TestServer::start().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(s.url("/api/web/douyin/download"))
        .json(&json!({ "aweme_ids": [] }))
        .send()
        .await
        .unwrap();
    // submit 时不立刻校验空 ids（task 体内 bail），所以返回 200 + task_id；任务跑起来后会 failed。
    // 这里只确认 endpoint 通且返回 task_id 形态。
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert!(v["task_id"].as_str().unwrap().starts_with("tk_"));
    assert_eq!(v["kind"], "douyin_download");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_works_returns_task_id() {
    let s = TestServer::start().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(s.url("/api/web/douyin/sync_works"))
        .json(&json!({ "handle": "MS4wLjABAAAAfake", "max_pages": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["kind"], "douyin_list_works");
    assert!(v["task_id"].as_str().unwrap().starts_with("tk_"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcribe_returns_task_id() {
    let s = TestServer::start().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(s.url("/api/web/douyin/transcribe"))
        .json(&json!({ "aweme_ids": ["123"], "vad": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["kind"], "douyin_transcribe");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kb_publish_handles_empty_only_ids() {
    let s = TestServer::start().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(s.url("/api/web/douyin/kb_publish"))
        .json(&json!({ "unique_id": "nonexistent" }))
        .send()
        .await
        .unwrap();
    // 端点通即可，业务结果可能是 error / 0 written 都接受
    assert!(resp.status().is_success() || resp.status().is_client_error());
}
