//! TextRefine 集成测试：起本地 axum mock 充当 OpenAI 兼容 LLM 端点，验证
//! 解析 / 重试 / 元信息写入 / 失败列表进 output。不依赖真实 vLLM / 抖音。

mod common;
use common::TestServer;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// mock LLM 状态：记录被调用次数，按脚本决定每次返回成功 / 503。
#[derive(Clone)]
struct MockState {
    calls: Arc<AtomicUsize>,
    /// 对前 `fail_first` 次调用返回 503（测重试）。
    fail_first: usize,
}

async fn chat_completions(
    State(st): State<MockState>,
    Json(_body): Json<Value>,
) -> (axum::http::StatusCode, Json<Value>) {
    let n = st.calls.fetch_add(1, Ordering::SeqCst);
    if n < st.fail_first {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "overloaded"})),
        );
    }
    (
        axum::http::StatusCode::OK,
        Json(json!({
            "choices": [{
                "message": { "role": "assistant", "content": "整理后的正文。\n\n## 小结\n讲了重点。" }
            }]
        })),
    )
}

/// 起 mock LLM，返回 base_url（形如 http://127.0.0.1:PORT/v1）+ 调用计数器。
async fn start_mock_llm(fail_first: usize) -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let state = MockState {
        calls: calls.clone(),
        fail_first,
    };
    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/v1"), calls)
}

fn write_transcript(ws: &std::path::Path, aweme_id: &str, text: &str) {
    let dir = ws.join("douyin").join("transcripts");
    std::fs::create_dir_all(&dir).unwrap();
    let t = json!({
        "aweme_id": aweme_id,
        "text": text,
        "segments": [],
        "has_segments": false,
        "asr_model": "sense-voice",
        "transcribed_at": "2026-06-10T00:00:00Z",
    });
    std::fs::write(dir.join(format!("{aweme_id}.json")), t.to_string()).unwrap();
}

async fn poll_terminal(client: &reqwest::Client, url: &str) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
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

// 两个场景合到一个测试里顺序跑：`LLM_BASE_URL` 是进程级环境变量，并行测试会互相
// 覆盖导致重试计数 flaky。合并后串行，env 始终指向当前场景的 mock。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn text_refine_end_to_end() {
    retries_then_succeeds_and_writes_metadata().await;
    missing_transcript_goes_to_failures().await;
}

async fn retries_then_succeeds_and_writes_metadata() {
    // mock 第一次 503，第二次成功 → 验证重试。
    let (base, calls) = start_mock_llm(1).await;
    std::env::set_var("LLM_BASE_URL", &base);
    std::env::set_var("LLM_MODEL", "mock-qwen");
    std::env::remove_var("LLM_API_KEY");

    let s = TestServer::start().await;
    write_transcript(s.dir.path(), "111", "嗯 这个 然后 我们今天讲一下 那个 数字");

    let client = reqwest::Client::new();
    let resp = client
        .post(s.url("/api/web/douyin/refine"))
        .json(&json!({ "aweme_ids": ["111"] }))
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
    assert_eq!(out["total"], 1);
    assert_eq!(out["refined"], 1);
    assert_eq!(out["failed"], 0);
    assert_eq!(out["model"], "mock-qwen");
    assert_eq!(out["prompt_version"], "v1");
    assert!(out["prompt_hash"].as_str().unwrap().len() == 16);

    // 重试发生：mock 被调 2 次（首次 503 + 重试成功）。
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // 整理稿落盘且带元信息。
    let refined_path = s.dir.path().join("douyin").join("refined").join("111.json");
    let refined: Value =
        serde_json::from_str(&std::fs::read_to_string(&refined_path).unwrap()).unwrap();
    assert_eq!(refined["model"], "mock-qwen");
    assert_eq!(refined["prompt_version"], "v1");
    assert!(refined["refined_text"]
        .as_str()
        .unwrap()
        .contains("## 小结"));
    assert!(refined["refined_at"].as_str().unwrap().contains("2026"));

    std::env::remove_var("LLM_BASE_URL");
    std::env::remove_var("LLM_MODEL");
}

async fn missing_transcript_goes_to_failures() {
    let (base, _calls) = start_mock_llm(0).await;
    std::env::set_var("LLM_BASE_URL", &base);
    std::env::set_var("LLM_MODEL", "mock-qwen");

    let s = TestServer::start().await;
    // 不写 222 的转写 → 应进 failures，不拖垮任务（仍 succeeded，failed=1）。
    let client = reqwest::Client::new();
    let resp = client
        .post(s.url("/api/web/douyin/refine"))
        .json(&json!({ "aweme_ids": ["222"] }))
        .send()
        .await
        .unwrap();
    let task_id = resp.json::<Value>().await.unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let v = poll_terminal(&client, &s.url(&format!("/api/web/tasks/{task_id}"))).await;
    assert_eq!(v["state"], "succeeded", "task={v}");
    assert_eq!(v["output"]["failed"], 1);
    assert_eq!(v["output"]["refined"], 0);
    assert_eq!(v["output"]["failures"][0]["aweme_id"], "222");

    std::env::remove_var("LLM_BASE_URL");
    std::env::remove_var("LLM_MODEL");
}
