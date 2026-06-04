mod common;
use common::TestServer;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_upserts_browser_session() {
    let s = TestServer::start().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(s.url("/api/browser/hello"))
        .json(&json!({
            "session_id": "test-sess-1",
            "user_agent": "Mozilla/5.0",
            "extension_version": "0.1.0"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert!(v["server_version"].is_string());
    assert!(v["accepted_at"].is_string());

    // 再发一次（同 session_id），应仍 200
    let resp2 = client
        .post(s.url("/api/browser/hello"))
        .json(&json!({
            "session_id": "test-sess-1",
            "user_agent": "Mozilla/5.0",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn url_endpoint_classifies() {
    let s = TestServer::start().await;
    let client = reqwest::Client::new();
    // 命中博主主页
    let resp = client
        .post(s.url("/api/browser/url"))
        .json(&json!({
            "session_id": "u1",
            "url": "https://www.douyin.com/user/MS4wLjABAAAAabcXYZ"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["matched"], "creator_home");

    // 命中作品页
    let resp = client
        .post(s.url("/api/browser/url"))
        .json(&json!({
            "session_id": "u1",
            "url": "https://www.douyin.com/video/7123456789012345678"
        }))
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["matched"], "work");

    // 不命中
    let resp = client
        .post(s.url("/api/browser/url"))
        .json(&json!({
            "session_id": "u1",
            "url": "https://example.com/foo"
        }))
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = resp.json().await.unwrap();
    assert!(v["matched"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cookie_endpoint_parses_and_upserts() {
    let s = TestServer::start().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(s.url("/api/browser/cookie"))
        .json(&json!({
            "session_id": "u1",
            "raw_header": "msToken=abc; ttwid=def; other=xx",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["accepted"], true);
    assert_eq!(v["fields_count"], 3);
    let req: Vec<String> = v["has_required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    assert!(req.contains(&"msToken".to_string()));
    assert!(req.contains(&"ttwid".to_string()));
    assert!(!req.contains(&"sessionid_ss".to_string()));

    // 第二次推 cookie 覆盖
    let resp = client
        .post(s.url("/api/browser/cookie"))
        .json(&json!({
            "raw_header": "msToken=zzz; ttwid=yyy; sessionid_ss=ww",
        }))
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["fields_count"], 3);
    let req: Vec<String> = v["has_required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    assert!(req.contains(&"sessionid_ss".to_string()));
}
