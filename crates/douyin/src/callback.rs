//! 持久 callback 队列（P1）。
//!
//! 任务终态通知不再"发一次失败就丢"：worker 先把 callback 落盘成 `<task_id>.callback.json`
//! （pending），再尝试投递；成功标 delivered，失败留 pending 由 `callback-flush`（或定时
//! 调用）按指数退避补发。修掉设计 §4.4 欠债——worker 内重试全失败 / worker 提前崩溃 →
//! 通知永久丢失，任务完成但发起方永远不知道。
//!
//! 状态机：pending → delivered（送达）/ pending →（attempt 累加 + 退避）→ … →
//! failed（超 MAX_ATTEMPTS 放弃）。callback_id == task_id（一任务一回调，重入队覆盖）。

use anyhow::{Context, Result};
use custom_utils::trace::{self, SpanRecord, SpanStatus, TraceContext};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// gateway 回调地址（本机 LAN，与 alarm-server 同款 hardcode）。
pub const GATEWAY_CALLBACK_URL: &str = "http://127.0.0.1:9001/messages";
/// 超过此投递次数仍失败则标 failed，停止补发。
const MAX_ATTEMPTS: u32 = 8;
/// 退避基数（秒）：第 n 次失败后等 BASE * 2^min(n,6)。
const BASE_BACKOFF_SECS: i64 = 30;

/// 持久化的 callback 记录。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CallbackRecord {
    pub callback_id: String,
    pub task_id: String,
    pub kind: String,
    pub delivery_handle: String,
    /// 回调 payload：至少含 task_id，按任务类型可含 unique_id / session_id。
    pub payload: Map<String, Value>,
    /// pending | delivered | failed
    pub state: String,
    pub attempt: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub next_retry_at: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub delivered_at: Option<String>,
    /// 提交时捕获的 W3C traceparent（来自 zero），用于跨异步续接同一条 trace。
    #[serde(default)]
    pub trace_context: Option<String>,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn callback_path(task_dir: &Path, task_id: &str) -> PathBuf {
    task_dir.join(format!("{task_id}.callback.json"))
}

fn trace_path(task_dir: &Path, task_id: &str) -> PathBuf {
    task_dir.join(format!("{task_id}.trace"))
}

/// 写提交时捕获的 traceparent 侧文件（由 `run_submit_job` 在 submit 后调用）。
/// 与按 kind 改 Job 结构相比，侧文件对所有任务类型统一生效、零侵入。
pub fn write_trace(task_dir: &Path, task_id: &str, traceparent: &str) -> Result<()> {
    atomic_write(&trace_path(task_dir, task_id), traceparent)
}

/// 读侧文件里的 traceparent；不存在 / 空则 None。
fn read_trace(task_dir: &Path, task_id: &str) -> Option<String> {
    std::fs::read_to_string(trace_path(task_dir, task_id))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// callback 投递成功时记「抖音任务完成」span（续用提交时的 trace_id）。
fn record_done_span(rec: &CallbackRecord) {
    let Some(tp) = rec.trace_context.as_deref() else {
        return;
    };
    let Some(remote) = TraceContext::from_traceparent(tp) else {
        return;
    };
    let ctx = TraceContext::continued(remote.trace_id, remote.span_id);
    let start = chrono::DateTime::parse_from_rfc3339(&rec.created_at)
        .map(|t| t.timestamp_millis())
        .unwrap_or_else(|_| trace::now_ms());
    let status = if rec.kind.contains("failed") {
        SpanStatus::Error(rec.kind.clone())
    } else {
        SpanStatus::Ok
    };
    trace::record_span(SpanRecord {
        trace_id: ctx.trace_id,
        span_id: ctx.span_id,
        parent_span_id: ctx.parent_span_id,
        service: String::new(),
        kind: "douyin_done".to_string(),
        flow_name: Some("抖音任务完成".to_string()),
        start_ms: start,
        end_ms: trace::now_ms(),
        status,
        summary: json!({ "task_id": rec.task_id, "callback_kind": rec.kind }),
        detail: Value::Object(rec.payload.clone()),
        request_body: None,
        response_body: None,
        body_truncated: false,
        links: Vec::new(),
    });
}

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content).with_context(|| format!("写临时文件 {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("替换 {}", path.display()))?;
    Ok(())
}

/// 读 callback 记录；不存在返回 None。
pub fn read(task_dir: &Path, task_id: &str) -> Result<Option<CallbackRecord>> {
    let p = callback_path(task_dir, task_id);
    if !p.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&p)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

fn write(task_dir: &Path, rec: &CallbackRecord) -> Result<()> {
    atomic_write(
        &callback_path(task_dir, &rec.task_id),
        &serde_json::to_string(rec)?,
    )
}

/// 入队一条 pending callback（worker 终态调用）。重入队即覆盖。
pub fn enqueue(
    task_dir: &Path,
    task_id: &str,
    kind: &str,
    delivery_handle: &str,
    payload: Map<String, Value>,
) -> Result<()> {
    let rec = CallbackRecord {
        callback_id: task_id.to_string(),
        task_id: task_id.to_string(),
        kind: kind.to_string(),
        delivery_handle: delivery_handle.to_string(),
        payload,
        state: "pending".into(),
        attempt: 0,
        last_error: None,
        next_retry_at: None,
        created_at: now(),
        delivered_at: None,
        trace_context: read_trace(task_dir, task_id),
    };
    write(task_dir, &rec)
}

/// 构造 POST gateway 的 body（与历史 inline 实现一致）。
fn build_body(rec: &CallbackRecord) -> Value {
    json!({
        "sender_id": "system:callback",
        "text": format!("<callback kind=\"{}\" task_id=\"{}\"/>", rec.kind, rec.task_id),
        "metadata": {
            "callback": {
                "kind": rec.kind,
                "payload": Value::Object(rec.payload.clone())
            },
            "delivery_handle": rec.delivery_handle
        }
    })
}

async fn post_once(url: &str, body: &Value, trace_context: Option<&str>) -> Result<()> {
    let client = reqwest::Client::new();
    let mut req = client
        .post(url)
        .json(body)
        .timeout(std::time::Duration::from_secs(10));
    // 把提交时的 trace 透传给 gateway，zero 据此续接同一条 trace。
    if let Some(tp) = trace_context {
        req = req.header("traceparent", tp);
    }
    let resp = req.send().await?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("gateway HTTP {}", resp.status()))
    }
}

fn backoff_next(attempt: u32) -> String {
    let exp = attempt.min(6); // cap 2^6 = 64
    let secs = BASE_BACKOFF_SECS * (1i64 << exp);
    (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339()
}

/// 投递成功后的记录变更（纯函数，便于测试）。
fn apply_success(rec: &mut CallbackRecord) {
    rec.state = "delivered".into();
    rec.delivered_at = Some(now());
    rec.last_error = None;
    rec.next_retry_at = None;
}

/// 投递失败后的记录变更：累加 attempt，达上限标 failed，否则设退避（纯函数，便于测试）。
fn apply_failure(rec: &mut CallbackRecord, err: String) {
    rec.attempt += 1;
    rec.last_error = Some(err);
    if rec.attempt >= MAX_ATTEMPTS {
        rec.state = "failed".into();
        rec.next_retry_at = None;
    } else {
        rec.state = "pending".into();
        rec.next_retry_at = Some(backoff_next(rec.attempt));
    }
}

/// 投递一次：成功标 delivered 并同步 status.notified；失败累加 attempt + 退避。
/// 返回是否已送达。记录不存在或已 delivered 时分别返回 false/true。
pub async fn deliver(task_dir: &Path, task_id: &str, url: &str) -> Result<bool> {
    let Some(mut rec) = read(task_dir, task_id)? else {
        return Ok(false);
    };
    if rec.state == "delivered" {
        return Ok(true);
    }
    let body = build_body(&rec);
    match post_once(url, &body, rec.trace_context.as_deref()).await {
        Ok(()) => {
            apply_success(&mut rec);
            write(task_dir, &rec)?;
            mark_status_notified(task_dir, task_id);
            crate::events::append(task_dir, task_id, "callback.delivered", None);
            record_done_span(&rec);
            Ok(true)
        }
        Err(e) => {
            apply_failure(&mut rec, e.to_string());
            // 仅在彻底放弃时记一条 failed 事件，避免每次退避都刷屏。
            if rec.state == "failed" {
                crate::events::append(
                    task_dir,
                    task_id,
                    "callback.failed",
                    Some(json!({ "attempt": rec.attempt, "last_error": rec.last_error })),
                );
            }
            write(task_dir, &rec)?;
            Ok(false)
        }
    }
}

/// worker 终态调用：入队后当场短重试几次（每次间隔 5s），尽量立即送达。
/// 未当场送达则记录留 pending，由 `flush` 后续补发。返回是否当场送达。
pub async fn enqueue_and_deliver(
    task_dir: &Path,
    task_id: &str,
    kind: &str,
    delivery_handle: &str,
    payload: Map<String, Value>,
    url: &str,
) -> Result<bool> {
    enqueue(task_dir, task_id, kind, delivery_handle, payload)?;
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
        if deliver(task_dir, task_id, url).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 扫描 task_dir 下所有未送达 callback，对到期（next_retry_at ≤ now 或未设）的各投递一次。
/// 返回 (delivered, pending, failed) 计数。
pub async fn flush(task_dir: &Path, url: &str) -> Result<(usize, usize, usize)> {
    let now_dt = chrono::Utc::now();
    let mut task_ids: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(task_dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Some(task_id) = name.strip_suffix(".callback.json") {
                task_ids.push(task_id.to_string());
            }
        }
    }
    let (mut delivered, mut pending, mut failed) = (0usize, 0usize, 0usize);
    for task_id in task_ids {
        let Some(rec) = read(task_dir, &task_id)? else {
            continue;
        };
        match rec.state.as_str() {
            "delivered" => continue,
            "failed" => {
                failed += 1;
                continue;
            }
            _ => {}
        }
        let due = match &rec.next_retry_at {
            None => true,
            Some(ts) => chrono::DateTime::parse_from_rfc3339(ts)
                .map(|t| now_dt >= t.with_timezone(&chrono::Utc))
                .unwrap_or(true),
        };
        if !due {
            pending += 1;
            continue;
        }
        if deliver(task_dir, &task_id, url).await? {
            delivered += 1;
        } else if read(task_dir, &task_id)?.map(|r| r.state).as_deref() == Some("failed") {
            failed += 1;
        } else {
            pending += 1;
        }
    }
    Ok((delivered, pending, failed))
}

/// 把 `<task_id>.status.json` 的 `notified` 置 true（generic Value 更新，跨任务类型通用——
/// flush 在 worker 进程外补发成功时也能同步状态，无需依赖具体 TaskStatus 结构）。
fn mark_status_notified(task_dir: &Path, task_id: &str) {
    let p = task_dir.join(format!("{task_id}.status.json"));
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return;
    };
    let Ok(mut v) = serde_json::from_str::<Value>(&raw) else {
        return;
    };
    if let Some(obj) = v.as_object_mut() {
        obj.insert("notified".into(), json!(true));
        if let Ok(s) = serde_json::to_string(&v) {
            let _ = atomic_write(&p, &s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        let id = C.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("douyin-cb-test-{}-{}", std::process::id(), id));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn sample_payload() -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("task_id".into(), json!("dyproc1"));
        m.insert("unique_id".into(), json!("82933463317"));
        m
    }

    #[test]
    fn enqueue_then_read_roundtrip() {
        let dir = tempdir();
        enqueue(
            &dir,
            "dyproc1",
            "douyin-process-done",
            "dh_x",
            sample_payload(),
        )
        .unwrap();
        let rec = read(&dir, "dyproc1").unwrap().unwrap();
        assert_eq!(rec.state, "pending");
        assert_eq!(rec.attempt, 0);
        assert_eq!(rec.kind, "douyin-process-done");
        assert_eq!(rec.payload["unique_id"], json!("82933463317"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_missing_returns_none() {
        let dir = tempdir();
        assert!(read(&dir, "nope").unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn enqueue_picks_up_trace_side_file() {
        let dir = tempdir();
        let tp = "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01";
        write_trace(&dir, "dyproc_t", tp).unwrap();
        enqueue(
            &dir,
            "dyproc_t",
            "douyin-process-done",
            "dh_x",
            sample_payload(),
        )
        .unwrap();
        let rec = read(&dir, "dyproc_t").unwrap().unwrap();
        assert_eq!(rec.trace_context.as_deref(), Some(tp));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_body_shape_matches_contract() {
        let rec = CallbackRecord {
            callback_id: "dyproc1".into(),
            task_id: "dyproc1".into(),
            kind: "douyin-process-done".into(),
            delivery_handle: "dh_x".into(),
            payload: sample_payload(),
            state: "pending".into(),
            attempt: 0,
            last_error: None,
            next_retry_at: None,
            created_at: now(),
            delivered_at: None,
            trace_context: None,
        };
        let body = build_body(&rec);
        assert_eq!(body["sender_id"], "system:callback");
        assert_eq!(body["metadata"]["callback"]["kind"], "douyin-process-done");
        assert_eq!(
            body["metadata"]["callback"]["payload"]["unique_id"],
            "82933463317"
        );
        assert_eq!(body["metadata"]["delivery_handle"], "dh_x");
    }

    #[test]
    fn apply_failure_backs_off_then_gives_up() {
        let mut rec = CallbackRecord {
            callback_id: "x".into(),
            task_id: "x".into(),
            kind: "k".into(),
            delivery_handle: "dh".into(),
            payload: Map::new(),
            state: "pending".into(),
            attempt: 0,
            last_error: None,
            next_retry_at: None,
            created_at: now(),
            delivered_at: None,
            trace_context: None,
        };
        // 前几次失败 → pending + 设退避
        apply_failure(&mut rec, "boom".into());
        assert_eq!(rec.state, "pending");
        assert_eq!(rec.attempt, 1);
        assert!(rec.next_retry_at.is_some());
        assert_eq!(rec.last_error.as_deref(), Some("boom"));
        // 撑到上限 → failed + 不再设退避
        while rec.attempt < MAX_ATTEMPTS {
            apply_failure(&mut rec, "boom".into());
        }
        assert_eq!(rec.state, "failed");
        assert!(rec.next_retry_at.is_none());
    }

    #[test]
    fn apply_success_marks_delivered() {
        let mut rec = CallbackRecord {
            callback_id: "x".into(),
            task_id: "x".into(),
            kind: "k".into(),
            delivery_handle: "dh".into(),
            payload: Map::new(),
            state: "pending".into(),
            attempt: 3,
            last_error: Some("prev".into()),
            next_retry_at: Some(now()),
            created_at: now(),
            delivered_at: None,
            trace_context: None,
        };
        apply_success(&mut rec);
        assert_eq!(rec.state, "delivered");
        assert!(rec.delivered_at.is_some());
        assert!(rec.last_error.is_none());
        assert!(rec.next_retry_at.is_none());
    }

    #[test]
    fn mark_status_notified_sets_field() {
        let dir = tempdir();
        std::fs::write(
            dir.join("dyproc1.status.json"),
            r#"{"task_id":"dyproc1","state":"succeeded","notified":false}"#,
        )
        .unwrap();
        mark_status_notified(&dir, "dyproc1");
        let raw = std::fs::read_to_string(dir.join("dyproc1.status.json")).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["notified"], json!(true));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn flush_skips_terminal_and_not_due() {
        let dir = tempdir();
        // delivered → 跳过
        let mut delivered_rec = CallbackRecord {
            callback_id: "d".into(),
            task_id: "dyproc_d".into(),
            kind: "k".into(),
            delivery_handle: "dh".into(),
            payload: Map::new(),
            state: "delivered".into(),
            attempt: 1,
            last_error: None,
            next_retry_at: None,
            created_at: now(),
            delivered_at: Some(now()),
            trace_context: None,
        };
        write(&dir, &delivered_rec).unwrap();
        // failed → 计 failed，不投递
        delivered_rec.task_id = "dyproc_f".into();
        delivered_rec.state = "failed".into();
        write(&dir, &delivered_rec).unwrap();
        // pending 但未到期（next_retry 在未来）→ 计 pending，不投递（无网络）
        let mut pend = delivered_rec.clone();
        pend.task_id = "dyproc_p".into();
        pend.state = "pending".into();
        pend.next_retry_at =
            Some((chrono::Utc::now() + chrono::Duration::seconds(3600)).to_rfc3339());
        write(&dir, &pend).unwrap();

        let (delivered, pending, failed) = flush(&dir, GATEWAY_CALLBACK_URL).await.unwrap();
        assert_eq!(delivered, 0);
        assert_eq!(pending, 1);
        assert_eq!(failed, 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
