//! Plan 5 阶段1：「下载 + ASR 识别」合并异步任务（process）。
//!
//! 每个 aweme_id：下载无水印 mp4 → 调 asr-server `from-source` 端点转写 →
//! 把 `{text, segments[]}` 落盘为 transcript 缓存 `douyin/transcripts/<aweme_id>.json`。
//! 与 list_works / download 同款 fire-and-forget 进程模型：submit 立返 task_id，
//! 后台 worker 进程逐个处理 + 增量 status，完成时携 delivery_handle POST gateway 回调。
//!
//! 完整性 / 幂等：transcript JSON 已存在则跳过（skipped），不重复下载/转写。
//! transcript 缓存随后由 `knowledge::run_publish_knowledge` 读取，回填进知识条目 md。
//!
//! asr-server 契约（streaming-speech `server/asr-server`）：
//!   POST {asr_url}  body {"source":"file:///abs/path.mp4","vad":bool}
//!   200 {"text":"...", "segments":[{"start":f64,"end":f64,"text":"..."}]?}
//!   segments 仅 vad=true 时存在。

use crate::api::DouyinClient;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 单条转写结果缓存（落 `douyin/transcripts/<aweme_id>.json`）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Transcript {
    pub aweme_id: String,
    pub text: String,
    #[serde(default)]
    pub segments: Vec<Segment>,
    pub has_segments: bool,
    pub asr_model: String,
    pub transcribed_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// process 任务状态（落盘 JSON，原子替换）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaskStatus {
    pub task_id: String,
    pub state: String,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub results: Vec<ItemResult>,
    pub updated_at: String,
    #[serde(default)]
    pub notified: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ItemResult {
    pub aweme_id: String,
    pub state: String,
    pub downloaded: bool,
    pub transcribed: bool,
    pub has_segments: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 作业描述（submit 写、worker 读）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Job {
    pub task_id: String,
    pub ids: Vec<String>,
    pub cookie_file: PathBuf,
    pub out_dir: PathBuf,
    pub transcript_dir: PathBuf,
    pub asr_url: String,
    pub asr_model: String,
    pub vad: bool,
    #[serde(default)]
    pub delivery_handle: Option<String>,
    #[serde(default)]
    pub unique_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn status_path(task_dir: &Path, task_id: &str) -> PathBuf {
    task_dir.join(format!("{task_id}.status.json"))
}

fn job_path(task_dir: &Path, task_id: &str) -> PathBuf {
    task_dir.join(format!("{task_id}.job.json"))
}

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content).with_context(|| format!("写临时文件 {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("替换 {}", path.display()))?;
    Ok(())
}

fn write_status(task_dir: &Path, st: &TaskStatus) -> Result<()> {
    atomic_write(
        &status_path(task_dir, &st.task_id),
        &serde_json::to_string(st)?,
    )
}

pub fn read_status(task_dir: &Path, task_id: &str) -> Result<Option<TaskStatus>> {
    let p = status_path(task_dir, task_id);
    if !p.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&p)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

fn transcript_path(transcript_dir: &Path, aweme_id: &str) -> PathBuf {
    transcript_dir.join(format!("{aweme_id}.json"))
}

/// 读单条 transcript 缓存（供 knowledge 回填复用）。
pub fn read_transcript(transcript_dir: &Path, aweme_id: &str) -> Option<Transcript> {
    let p = transcript_path(transcript_dir, aweme_id);
    let raw = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 入队：生成 `dyproc<ms>` task_id，落 job + 初始 status，spawn 脱离 worker，立即返回。
#[allow(clippy::too_many_arguments)]
pub fn submit(
    task_dir: &Path,
    out_dir: &Path,
    transcript_dir: &Path,
    cookie_file: &Path,
    ids: Vec<String>,
    asr_url: String,
    asr_model: String,
    vad: bool,
    delivery_handle: Option<String>,
    unique_id: Option<String>,
    session_id: Option<String>,
) -> Result<(TaskStatus, usize)> {
    std::fs::create_dir_all(task_dir)
        .with_context(|| format!("建任务目录 {}", task_dir.display()))?;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let task_id = format!("dyproc{ms}");

    // 预统计已转写的（幂等跳过）——供 submit 立即回 skipped_already_done。
    let already: usize = ids
        .iter()
        .filter(|id| transcript_path(transcript_dir, id).exists())
        .count();

    let job = Job {
        task_id: task_id.clone(),
        ids: ids.clone(),
        cookie_file: cookie_file.to_path_buf(),
        out_dir: out_dir.to_path_buf(),
        transcript_dir: transcript_dir.to_path_buf(),
        asr_url,
        asr_model,
        vad,
        delivery_handle,
        unique_id,
        session_id,
    };
    atomic_write(&job_path(task_dir, &task_id), &serde_json::to_string(&job)?)?;

    let st = TaskStatus {
        task_id: task_id.clone(),
        state: "queued".into(),
        total: ids.len(),
        done: 0,
        failed: 0,
        skipped: 0,
        results: vec![],
        updated_at: now(),
        notified: false,
    };
    write_status(task_dir, &st)?;

    #[cfg(not(test))]
    {
        let exe = std::env::current_exe().context("取当前可执行路径")?;
        std::process::Command::new(exe)
            .arg("process-worker")
            .arg("--task-dir")
            .arg(task_dir)
            .arg("--task-id")
            .arg(&task_id)
            .spawn()
            .context("spawn process worker")?;
    }

    Ok((st, already))
}

/// worker 入口：逐个 aweme_id 下载 + 转写，增量更新 status，完成发回调。
pub async fn run_worker(task_dir: &Path, task_id: &str) -> Result<()> {
    let job: Job = {
        let raw = std::fs::read_to_string(job_path(task_dir, task_id)).context("读 job 文件")?;
        serde_json::from_str(&raw).context("解析 job")?
    };
    std::fs::create_dir_all(&job.out_dir).ok();
    std::fs::create_dir_all(&job.transcript_dir).ok();

    let cookies = match crate::load_cookie_file(&job.cookie_file) {
        Ok(c) => c,
        Err(e) => {
            write_all_failed(task_dir, &job, format!("读 cookie 文件: {e}"))?;
            return Ok(());
        }
    };
    let client = match DouyinClient::from_cookies(&cookies) {
        Ok(c) => c,
        Err(e) => {
            write_all_failed(task_dir, &job, format!("cookie 不可用: {e}"))?;
            return Ok(());
        }
    };

    let mut st = TaskStatus {
        task_id: task_id.into(),
        state: "running".into(),
        total: job.ids.len(),
        done: 0,
        failed: 0,
        skipped: 0,
        results: vec![],
        updated_at: now(),
        notified: false,
    };
    write_status(task_dir, &st)?;

    let http = reqwest::Client::new();
    for id in &job.ids {
        // 幂等：已有 transcript 缓存则跳过下载+转写。
        if transcript_path(&job.transcript_dir, id).exists() {
            let has_seg = read_transcript(&job.transcript_dir, id)
                .map(|t| t.has_segments)
                .unwrap_or(false);
            st.skipped += 1;
            st.done += 1;
            st.results.push(ItemResult {
                aweme_id: id.clone(),
                state: "skipped".into(),
                downloaded: true,
                transcribed: true,
                has_segments: has_seg,
                error: None,
            });
            st.updated_at = now();
            write_status(task_dir, &st)?;
            continue;
        }

        let res = process_one(&client, &http, id, &job).await;
        match res {
            Ok(has_seg) => {
                st.done += 1;
                st.results.push(ItemResult {
                    aweme_id: id.clone(),
                    state: "succeeded".into(),
                    downloaded: true,
                    transcribed: true,
                    has_segments: has_seg,
                    error: None,
                });
            }
            Err(e) => {
                st.failed += 1;
                st.results.push(ItemResult {
                    aweme_id: id.clone(),
                    state: "failed".into(),
                    downloaded: false,
                    transcribed: false,
                    has_segments: false,
                    error: Some(e.to_string()),
                });
            }
        }
        st.updated_at = now();
        write_status(task_dir, &st)?;
    }

    st.state = if st.failed == 0 {
        "succeeded"
    } else if st.done == 0 {
        "failed"
    } else {
        "partial"
    }
    .into();
    st.updated_at = now();
    write_status(task_dir, &st)?;

    // 业务回调：通知 zero gateway，第二轮 LLM 周期接管（调 publish_knowledge 回填）。
    if let Some(handle) = &job.delivery_handle {
        let kind = if st.state == "failed" {
            "douyin-process-failed"
        } else {
            "douyin-process-done"
        };
        match post_gateway_callback(
            handle,
            kind,
            &st.task_id,
            job.unique_id.as_deref(),
            job.session_id.as_deref(),
        )
        .await
        {
            Ok(()) => {
                st.notified = true;
                st.updated_at = now();
                let _ = write_status(task_dir, &st);
                log::info!("[process callback] notified=true task_id={}", st.task_id);
            }
            Err(e) => {
                log::warn!("[process callback] post failed task_id={}: {e}", st.task_id);
            }
        }
    }
    Ok(())
}

/// 下载 mp4 + 调 asr-server 转写 + 落 transcript 缓存。返回 has_segments。
async fn process_one(
    client: &DouyinClient,
    http: &reqwest::Client,
    aweme_id: &str,
    job: &Job,
) -> Result<bool> {
    let mp4_path = crate::download::download_one(client, http, aweme_id, &job.out_dir)
        .await
        .context("下载 mp4")?;

    // 用绝对路径拼 file:// URI（worker 在宿主跑，路径 == asr-server 容器挂载路径）。
    let abs = std::fs::canonicalize(&mp4_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or(mp4_path);
    let source = format!("file://{abs}");

    let resp = http
        .post(&job.asr_url)
        .json(&serde_json::json!({ "source": source, "vad": job.vad }))
        .send()
        .await
        .context("调 asr-server")?;
    let status = resp.status();
    let body = resp.text().await.context("读 asr 响应")?;
    if !status.is_success() {
        anyhow::bail!(
            "asr-server {status}: {}",
            body.chars().take(200).collect::<String>()
        );
    }

    let parsed: AsrResponse = serde_json::from_str(&body).context("解析 asr 响应")?;
    let segments: Vec<Segment> = parsed
        .segments
        .unwrap_or_default()
        .into_iter()
        .map(|s| Segment {
            start: s.start,
            end: s.end,
            text: s.text,
        })
        .collect();
    let has_segments = !segments.is_empty();

    let transcript = Transcript {
        aweme_id: aweme_id.to_string(),
        text: parsed.text,
        segments,
        has_segments,
        asr_model: job.asr_model.clone(),
        transcribed_at: now(),
    };
    atomic_write(
        &transcript_path(&job.transcript_dir, aweme_id),
        &serde_json::to_string(&transcript)?,
    )?;
    Ok(has_segments)
}

#[derive(Deserialize)]
struct AsrResponse {
    text: String,
    #[serde(default)]
    segments: Option<Vec<AsrSegment>>,
}

#[derive(Deserialize)]
struct AsrSegment {
    start: f64,
    end: f64,
    text: String,
}

fn write_all_failed(task_dir: &Path, job: &Job, error: String) -> Result<()> {
    let st = TaskStatus {
        task_id: job.task_id.clone(),
        state: "failed".into(),
        total: job.ids.len(),
        done: 0,
        failed: job.ids.len(),
        skipped: 0,
        results: job
            .ids
            .iter()
            .map(|id| ItemResult {
                aweme_id: id.clone(),
                state: "failed".into(),
                downloaded: false,
                transcribed: false,
                has_segments: false,
                error: Some(error.clone()),
            })
            .collect(),
        updated_at: now(),
        notified: false,
    };
    write_status(task_dir, &st)
}

const GATEWAY_CALLBACK_URL: &str = "http://127.0.0.1:9001/messages";

async fn post_gateway_callback(
    delivery_handle: &str,
    kind: &str,
    task_id: &str,
    unique_id: Option<&str>,
    session_id: Option<&str>,
) -> Result<()> {
    let mut payload = serde_json::Map::new();
    payload.insert("task_id".to_string(), serde_json::json!(task_id));
    if let Some(unique_id) = unique_id.filter(|s| !s.trim().is_empty()) {
        payload.insert("unique_id".to_string(), serde_json::json!(unique_id));
    }
    if let Some(session_id) = session_id.filter(|s| !s.trim().is_empty()) {
        payload.insert("session_id".to_string(), serde_json::json!(session_id));
    }
    let body = serde_json::json!({
        "sender_id": "system:callback",
        "text": format!("<callback kind=\"{kind}\" task_id=\"{task_id}\"/>"),
        "metadata": {
            "callback": { "kind": kind, "payload": serde_json::Value::Object(payload) },
            "delivery_handle": delivery_handle
        }
    });
    let client = reqwest::Client::new();
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
        match client
            .post(GATEWAY_CALLBACK_URL)
            .json(&body)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => last_err = Some(anyhow::anyhow!("gateway HTTP {}", resp.status())),
            Err(e) => last_err = Some(e.into()),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("unknown post error")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        let id = C.fetch_add(1, Ordering::Relaxed);
        let p =
            std::env::temp_dir().join(format!("douyin-proc-test-{}-{}", std::process::id(), id));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn transcript_roundtrip() {
        let dir = tempdir();
        let t = Transcript {
            aweme_id: "7a".into(),
            text: "你好世界".into(),
            segments: vec![Segment {
                start: 0.0,
                end: 1.5,
                text: "你好".into(),
            }],
            has_segments: true,
            asr_model: "sense-voice".into(),
            transcribed_at: "2026-05-31T00:00:00Z".into(),
        };
        atomic_write(
            &transcript_path(&dir, "7a"),
            &serde_json::to_string(&t).unwrap(),
        )
        .unwrap();
        let back = read_transcript(&dir, "7a").unwrap();
        assert_eq!(back.text, "你好世界");
        assert_eq!(back.segments.len(), 1);
        assert!(back.has_segments);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_transcript_missing_returns_none() {
        let dir = tempdir();
        assert!(read_transcript(&dir, "nope").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn asr_response_parses_with_and_without_segments() {
        let with: AsrResponse =
            serde_json::from_str(r#"{"text":"a","segments":[{"start":0.0,"end":1.0,"text":"a"}]}"#)
                .unwrap();
        assert_eq!(with.segments.unwrap().len(), 1);
        let without: AsrResponse = serde_json::from_str(r#"{"text":"a"}"#).unwrap();
        assert!(without.segments.is_none());
    }

    #[test]
    fn status_roundtrip() {
        let dir = tempdir();
        let st = TaskStatus {
            task_id: "dyproc1".into(),
            state: "partial".into(),
            total: 2,
            done: 1,
            failed: 1,
            skipped: 0,
            results: vec![ItemResult {
                aweme_id: "7a".into(),
                state: "succeeded".into(),
                downloaded: true,
                transcribed: true,
                has_segments: true,
                error: None,
            }],
            updated_at: now(),
            notified: false,
        };
        write_status(&dir, &st).unwrap();
        let back = read_status(&dir, "dyproc1").unwrap().unwrap();
        assert_eq!(back.done, 1);
        assert_eq!(back.results.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_job_preserves_unique_id_and_session_id() {
        let dir = tempdir();
        let cookie = dir.join("cookie.json");
        let out_dir = dir.join("out");
        let transcript_dir = dir.join("transcripts");
        std::fs::write(&cookie, "{}").unwrap();
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::create_dir_all(&transcript_dir).unwrap();

        let (st, _) = submit(
            &dir,
            &out_dir,
            &transcript_dir,
            &cookie,
            vec!["123".to_string()],
            "http://127.0.0.1:8091/v1/audio/transcriptions/from-source".to_string(),
            "sense-voice".to_string(),
            true,
            Some("dh_test".to_string()),
            Some("82933463317".to_string()),
            Some("sess-123".to_string()),
        )
        .unwrap();

        let raw = std::fs::read_to_string(job_path(&dir, &st.task_id)).unwrap();
        let job: Job = serde_json::from_str(&raw).unwrap();
        assert_eq!(job.unique_id.as_deref(), Some("82933463317"));
        assert_eq!(job.session_id.as_deref(), Some("sess-123"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
