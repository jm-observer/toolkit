use serde_json::json;
use toolkit_core::{migrate, open_pool};
use toolkit_tasks::{status, submit, EchoTask, Registry};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn echo_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_pool(&dir.path().join("t.db")).unwrap();
    migrate(&pool).unwrap();

    let mut reg = Registry::new();
    reg.register::<EchoTask>();

    let id = submit(
        &reg,
        &pool,
        dir.path(),
        "echo",
        json!({"message": "hello", "delay_ms": 100}),
        None,
        None,
    )
    .unwrap();

    // 轮询至 terminal
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let s = status(&pool, &id).unwrap().unwrap();
        if s.state != "queued" && s.state != "running" {
            assert_eq!(s.state, "succeeded", "state={} err={:?}", s.state, s.error);
            assert_eq!(s.output.unwrap()["message"], "hello");
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("timeout, last state={}", s.state);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_kind_fails_submit() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_pool(&dir.path().join("t.db")).unwrap();
    migrate(&pool).unwrap();
    let reg = Registry::new();
    let err = submit(
        &reg,
        &pool,
        dir.path(),
        "nope",
        serde_json::json!({}),
        None,
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown kind"));
}
