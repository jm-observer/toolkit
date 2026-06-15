//! AudioForge 集成测试：起本地 axum mock 充当上游 CosyVoice2 `/tts`（返回最小合法
//! WAV bytes），验证逐句打包 / manifest 生成 / 失败重试 / 下载途径 / 防路径穿越。
//! 不依赖真实 TTS。

mod common;
use common::TestServer;

use axum::extract::State;
use axum::routing::post;
use axum::Router;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 构造一个最小合法 PCM WAV（16-bit 单声道 16kHz，含 `samples` 个样本）。
fn minimal_wav(samples: u32) -> Vec<u8> {
    let sample_rate: u32 = 16000;
    let channels: u16 = 1;
    let bits: u16 = 16;
    let byte_rate = sample_rate * channels as u32 * (bits / 8) as u32;
    let data_len = samples * channels as u32 * (bits / 8) as u32;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&sample_rate.to_le_bytes());
    v.extend_from_slice(&byte_rate.to_le_bytes());
    v.extend_from_slice(&(channels * bits / 8).to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    v.extend(std::iter::repeat_n(0u8, data_len as usize));
    v
}

#[derive(Clone)]
struct MockTts {
    calls: Arc<AtomicUsize>,
    /// 前 `fail_first` 次调用返回 503（测重试）。
    fail_first: usize,
    /// 文本含该子串的句子永久失败（测 failures 不拖垮整批）。
    fail_text: Option<String>,
}

async fn tts_handler(
    State(st): State<MockTts>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");

    if let Some(bad) = &st.fail_text {
        if text.contains(bad.as_str()) {
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response();
        }
    }

    let n = st.calls.fetch_add(1, Ordering::SeqCst);
    if n < st.fail_first {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "overloaded").into_response();
    }
    // 文本长度决定样本数（让不同句时长可区分）。
    let samples = 16000 + (text.len() as u32 * 100);
    (
        axum::http::StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "audio/wav")],
        minimal_wav(samples),
    )
        .into_response()
}

async fn start_mock_tts(fail_first: usize, fail_text: Option<&str>) -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let state = MockTts {
        calls: calls.clone(),
        fail_first,
        fail_text: fail_text.map(str::to_string),
    };
    let app = Router::new()
        .route("/tts", post(tts_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), calls)
}

async fn poll_terminal(client: &reqwest::Client, url: &str) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let v: Value = client.get(url).send().await.unwrap().json().await.unwrap();
        let state = v["state"].as_str().unwrap_or("");
        if state != "queued" && state != "running" {
            return v;
        }
        if std::time::Instant::now() > deadline {
            panic!("timeout, last={v}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

// 串行跑全部场景：TTS_BASE_URL 是进程级 env，并行测试会互相覆盖导致 flaky。
// 合并后串行，env 始终指向当前场景。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audio_forge_end_to_end() {
    fails_fast_without_upstream().await;
    happy_path_packages_and_manifest_and_download().await;
    retry_then_one_failure_does_not_break_batch().await;
}

async fn happy_path_packages_and_manifest_and_download() {
    let (base, calls) = start_mock_tts(0, None).await;
    std::env::set_var("TTS_BASE_URL", &base);

    let s = TestServer::start().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(s.url("/api/web/audio/forge"))
        .json(&json!({
            "package_name": "数字入门",
            "topic": "数字",
            "language": "en",
            "voice_id": "edge_en_female",
            "tts_params": { "speed": 1.0 },
            "sentences": [
                { "text": "One.", "translation": "一" },
                { "text": "Two.", "translation": "二", "voice_id": "edge_en_male" }
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let task_id = resp.json::<Value>().await.unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();

    let v = poll_terminal(&client, &s.url(&format!("/api/web/tasks/{task_id}"))).await;
    assert_eq!(v["state"], "succeeded", "task={v}");
    let out = &v["output"];
    assert_eq!(out["total"], 2);
    assert_eq!(out["generated"], 2);
    assert_eq!(out["failed"], 0);
    assert_eq!(out["source"], "manual");
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let package_id = out["package_id"].as_str().unwrap().to_string();

    // manifest 落盘且结构正确。
    let manifest_disk = s
        .dir
        .path()
        .join("audioforge")
        .join(&package_id)
        .join("manifest.json");
    let m: Value = serde_json::from_str(&std::fs::read_to_string(&manifest_disk).unwrap()).unwrap();
    assert_eq!(m["manifest_version"], 1);
    assert_eq!(m["package_name"], "数字入门");
    assert_eq!(m["topic"], "数字");
    assert_eq!(m["sentences"].as_array().unwrap().len(), 2);
    assert_eq!(m["sentences"][0]["audio_file"], "001.wav");
    assert_eq!(m["sentences"][0]["translation"], "一");
    // 逐句 voice 覆盖生效。
    assert_eq!(m["sentences"][1]["voice_id"], "edge_en_male");
    // WAV 时长被解析（>1.0s，因样本数 >16000）。
    assert!(m["sentences"][0]["duration"].as_f64().unwrap() >= 1.0);

    // 下载途径：manifest 可经 HTTP 拉取。
    let durl = s.url(&format!("/api/web/audio/forge/{package_id}/manifest.json"));
    let dm: Value = client
        .get(&durl)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(dm["package_id"], package_id);

    // 下载音频文件。
    let aurl = s.url(&format!("/api/web/audio/forge/{package_id}/001.wav"));
    let aresp = client.get(&aurl).send().await.unwrap();
    assert_eq!(aresp.status(), 200);
    assert_eq!(
        aresp.headers()["content-type"],
        "audio/wav",
        "音频 content-type"
    );
    let abytes = aresp.bytes().await.unwrap();
    assert_eq!(&abytes[0..4], b"RIFF");

    // 防路径穿越：非法段被拒。
    let bad = client
        .get(s.url(&format!(
            "/api/web/audio/forge/{package_id}/..%2f..%2fsecret"
        )))
        .send()
        .await
        .unwrap();
    assert!(bad.status() == 400 || bad.status() == 404, "穿越应被拒");

    std::env::remove_var("TTS_BASE_URL");
}

async fn retry_then_one_failure_does_not_break_batch() {
    // 首次 503 → 重试成功；含 "BAD" 的句子永久失败。
    let (base, _calls) = start_mock_tts(1, Some("BAD")).await;
    std::env::set_var("TTS_BASE_URL", &base);

    let s = TestServer::start().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(s.url("/api/web/audio/forge"))
        .json(&json!({
            "package_name": "混合",
            "voice_id": "v1",
            "sentences": [
                { "text": "Good one." },
                { "text": "This is BAD." },
                { "text": "Another good one." }
            ]
        }))
        .send()
        .await
        .unwrap();
    let task_id = resp.json::<Value>().await.unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();

    let v = poll_terminal(&client, &s.url(&format!("/api/web/tasks/{task_id}"))).await;
    // 单句失败不拖垮整批：任务仍 succeeded，failed=1。
    assert_eq!(v["state"], "succeeded", "task={v}");
    let out = &v["output"];
    assert_eq!(out["total"], 3);
    assert_eq!(out["generated"], 2);
    assert_eq!(out["failed"], 1);
    assert_eq!(out["failures"][0]["index"], 2);
    assert!(out["failures"][0]["text"].as_str().unwrap().contains("BAD"));

    std::env::remove_var("TTS_BASE_URL");
}

/// 未配置 TTS_BASE_URL 时任务提交后立即 failed（不空跑）。
async fn fails_fast_without_upstream() {
    std::env::remove_var("TTS_BASE_URL");
    let s = TestServer::start().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(s.url("/api/web/audio/forge"))
        .json(&json!({
            "package_name": "x",
            "voice_id": "v1",
            "sentences": [{ "text": "One." }]
        }))
        .send()
        .await
        .unwrap();
    let task_id = resp.json::<Value>().await.unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let v = poll_terminal(&client, &s.url(&format!("/api/web/tasks/{task_id}"))).await;
    assert_eq!(v["state"], "failed", "task={v}");
    assert!(v["error"].as_str().unwrap().contains("TTS_BASE_URL"));
}
