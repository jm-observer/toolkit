mod common;
use common::TestServer;

/// TTS 上游未配置（`TTS_BASE_URL` 未设）时，两个代理端点都返回明确的 503 + 提示。
/// 这是「未配置时返回明确的 503/错误 JSON」契约的回归锁。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audio_endpoints_503_when_unconfigured() {
    // 确保上游未配置（其它测试不设这个变量；本进程内显式清掉以防万一）。
    std::env::remove_var("TTS_BASE_URL");

    let s = TestServer::start().await;

    // GET /voices
    let resp = reqwest::get(s.url("/api/web/audio/voices")).await.unwrap();
    assert_eq!(resp.status(), 503);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["error"], "TTS upstream not configured");
    assert!(v["hint"].as_str().unwrap().contains("TTS_BASE_URL"));

    // POST /tts
    let resp = reqwest::Client::new()
        .post(s.url("/api/web/audio/tts"))
        .json(&serde_json::json!({ "text": "你好", "voice_id": "edge_yunjian" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["error"], "TTS upstream not configured");
}
