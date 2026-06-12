//! Plan 5 阶段1：「下载 + ASR 识别」合并异步任务（process）。
//!
//! 每个 aweme_id：下载无水印 mp4 → 把 mp4 字节 multipart 上传给 FunASR /transcribe →
//! 把 `{text, segments[]}` 落盘为 transcript 缓存 `douyin/transcripts/<aweme_id>.json`。
//! 与 list_works / download 同款 fire-and-forget 进程模型：submit 立返 task_id，
//! 后台 worker 进程逐个处理 + 增量 status，完成时携 delivery_handle POST gateway 回调。
//!
//! 完整性 / 幂等：transcript JSON 已存在则跳过（skipped），不重复下载/转写。
//! transcript 缓存随后由 `knowledge::run_publish_knowledge` 读取，回填进知识条目 md。
//!
//! ASR 调用通过 `asr-client` crate 完成；端点契约权威源是
//! `streaming-speech/docs/asr-transcribe-api.md`。本模块不再直接拼 multipart 或
//! 解析响应——把所有 FunASR 对接收敛在 asr-client 一处。

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
    /// worker 存活证明：running 期间周期性刷新。与 `updated_at`（进度变更时刻）区分——
    /// reap 据此判定 stale（worker 崩了 status 永远停在 running 而心跳不再更新）。
    /// `default` 保证旧 status 文件（无此字段）仍可反序列化。
    #[serde(default)]
    pub heartbeat_at: Option<String>,
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

/// 重启一个任务：把 status 标回 queued 并重 spawn worker。worker 会重建进度，
/// 已完成（有 transcript 缓存）的 item 自动 skip——靠幂等续传，不重下已完成内容。
/// 返回 None 表示 job 文件不存在（无从重启）。
pub fn retry(task_dir: &Path, task_id: &str) -> Result<Option<TaskStatus>> {
    if !job_path(task_dir, task_id).exists() {
        return Ok(None);
    }
    clear_cancel(task_dir, task_id); // 重跑前清掉残留 cancel 标志，避免一启动又被取消
    if let Some(mut st) = read_status(task_dir, task_id)? {
        st.state = "queued".into();
        st.updated_at = now();
        st.heartbeat_at = Some(now());
        st.notified = false;
        write_status(task_dir, &st)?;
    }
    spawn_worker(task_dir, task_id)?;
    read_status(task_dir, task_id)
}

/// 扫描并重启心跳超时（stale）的 running 任务。返回被 reap 的 task_id 列表。
/// 无 daemon 模型下的恢复入口：由 `process-reap` 命令（或定时调用）触发。
pub fn reap(task_dir: &Path, stale_secs: i64) -> Result<Vec<String>> {
    let ids = stale_running_task_ids(task_dir, stale_secs)?;
    for id in &ids {
        retry(task_dir, id)?;
    }
    Ok(ids)
}

/// 列出 task_dir 下所有 stale 的 running process 任务 id。
fn stale_running_task_ids(task_dir: &Path, stale_secs: i64) -> Result<Vec<String>> {
    let now = chrono::Utc::now();
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(task_dir) {
        Ok(r) => r,
        Err(_) => return Ok(out),
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        // 只认 process 任务：文件名 <task_id>.status.json 且 task_id 以 dyproc 开头。
        let Some(task_id) = name.strip_suffix(".status.json") else {
            continue;
        };
        if !task_id.starts_with("dyproc") {
            continue;
        }
        // 解析失败（如 download 任务的 status 结构不同）自然跳过。
        let Ok(Some(st)) = read_status(task_dir, task_id) else {
            continue;
        };
        if is_stale_running(&st, stale_secs, now) {
            out.push(task_id.to_string());
        }
    }
    Ok(out)
}

/// 判定一个任务是否 stale running：state==running 且心跳（缺则退化用 updated_at）
/// 距今 ≥ stale_secs。心跳时间无法解析时保守判定为非 stale（不误杀）。
fn is_stale_running(st: &TaskStatus, stale_secs: i64, now: chrono::DateTime<chrono::Utc>) -> bool {
    if st.state != "running" {
        return false;
    }
    let ts = st.heartbeat_at.as_deref().unwrap_or(&st.updated_at);
    let Ok(hb) = chrono::DateTime::parse_from_rfc3339(ts) else {
        return false;
    };
    (now - hb.with_timezone(&chrono::Utc)).num_seconds() >= stale_secs
}

fn cancel_flag_path(task_dir: &Path, task_id: &str) -> PathBuf {
    task_dir.join(format!("{task_id}.cancel"))
}

/// 请求取消任务：写 cancel 标志文件。worker 处理下一条前检查并转 cancelled。
/// 仅对 queued/running 任务有意义；返回 false 表示任务不存在或已终态（无可取消）。
pub fn cancel(task_dir: &Path, task_id: &str) -> Result<bool> {
    match read_status(task_dir, task_id)? {
        Some(st) if st.state == "queued" || st.state == "running" => {
            std::fs::write(cancel_flag_path(task_dir, task_id), "").context("写 cancel 标志")?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn is_cancelled(task_dir: &Path, task_id: &str) -> bool {
    cancel_flag_path(task_dir, task_id).exists()
}

fn clear_cancel(task_dir: &Path, task_id: &str) {
    let _ = std::fs::remove_file(cancel_flag_path(task_dir, task_id));
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

/// 已有 transcript 缓存则构造一条 skipped item（跨任务幂等）。
fn skipped_item(transcript_dir: &Path, id: &str) -> ItemResult {
    let has_seg = read_transcript(transcript_dir, id)
        .map(|t| t.has_segments)
        .unwrap_or(false);
    ItemResult {
        aweme_id: id.to_string(),
        state: "skipped".into(),
        downloaded: true,
        transcribed: true,
        has_segments: has_seg,
        error: None,
    }
}

/// 按 ids 构建初始 item 账本：已有 transcript 缓存的标 skipped，其余 queued。
/// 去重从此看账本不看文件——submit 时一次性建全量账本，worker 消费。
fn build_ledger(transcript_dir: &Path, ids: &[String]) -> Vec<ItemResult> {
    ids.iter()
        .map(|id| {
            if transcript_path(transcript_dir, id).exists() {
                skipped_item(transcript_dir, id)
            } else {
                ItemResult {
                    aweme_id: id.clone(),
                    state: "queued".into(),
                    downloaded: false,
                    transcribed: false,
                    has_segments: false,
                    error: None,
                }
            }
        })
        .collect()
}

/// 从 item 账本重算任务级计数。done 含 succeeded + skipped。
fn recompute_counts(st: &mut TaskStatus) {
    st.total = st.results.len();
    st.skipped = st.results.iter().filter(|r| r.state == "skipped").count();
    st.failed = st.results.iter().filter(|r| r.state == "failed").count();
    st.done = st
        .results
        .iter()
        .filter(|r| r.state == "succeeded" || r.state == "skipped")
        .count();
}

/// item 是否已到成功终态（不再重做）。failed 不算终态——retry 可重跑。
fn is_terminal_item(state: &str) -> bool {
    state == "succeeded" || state == "skipped"
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

    let mut st = TaskStatus {
        task_id: task_id.clone(),
        state: "queued".into(),
        total: ids.len(),
        done: 0,
        failed: 0,
        skipped: 0,
        results: build_ledger(transcript_dir, &ids),
        updated_at: now(),
        heartbeat_at: None,
        notified: false,
    };
    recompute_counts(&mut st);
    write_status(task_dir, &st)?;
    crate::events::append(
        task_dir,
        &task_id,
        "job.created",
        Some(serde_json::json!({ "total": st.total, "skipped": st.skipped })),
    );

    spawn_worker(task_dir, &task_id)?;

    Ok((st, already))
}

/// spawn 脱离的 process worker 子进程（同一二进制的隐藏子命令）。父进程退出后子进程继续。
/// submit / retry / reap 共用。test 下不真正起进程。
fn spawn_worker(task_dir: &Path, task_id: &str) -> Result<()> {
    #[cfg(not(test))]
    {
        let exe = std::env::current_exe().context("取当前可执行路径")?;
        std::process::Command::new(exe)
            .arg("process-worker")
            .arg("--task-dir")
            .arg(task_dir)
            .arg("--task-id")
            .arg(task_id)
            .spawn()
            .context("spawn process worker")?;
    }
    #[cfg(test)]
    {
        let _ = (task_dir, task_id);
    }
    Ok(())
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

    // 账本驱动：复用 submit / 上次运行落下的 item 账本，只处理未完成项（resume 语义）。
    // 缺失时按 job.ids 重建（防御，正常 submit 已写入）。
    let mut st = match read_status(task_dir, task_id)? {
        Some(mut s) => {
            s.state = "running".into();
            s.heartbeat_at = Some(now());
            s.updated_at = now();
            s
        }
        None => TaskStatus {
            task_id: task_id.into(),
            state: "running".into(),
            total: job.ids.len(),
            done: 0,
            failed: 0,
            skipped: 0,
            results: build_ledger(&job.transcript_dir, &job.ids),
            updated_at: now(),
            heartbeat_at: Some(now()),
            notified: false,
        },
    };
    recompute_counts(&mut st);
    write_status(task_dir, &st)?;
    crate::events::append(task_dir, task_id, "job.started", None);

    let http = reqwest::Client::new();
    for i in 0..st.results.len() {
        // 账本去重：已到成功终态（succeeded/skipped）的不重做。
        if is_terminal_item(&st.results[i].state) {
            continue;
        }
        // 取消检查：处理每条前看 cancel 标志，命中则转 cancelled 干净退出。
        if is_cancelled(task_dir, task_id) {
            clear_cancel(task_dir, task_id);
            st.state = "cancelled".into();
            st.updated_at = now();
            write_status(task_dir, &st)?;
            crate::events::append(task_dir, task_id, "job.cancelled", None);
            log::info!("[process] cancelled task_id={task_id}");
            return Ok(());
        }
        let id = st.results[i].aweme_id.clone();
        // 跨任务幂等快速路径：submit 后才出现的 transcript（别的任务下好的）也跳过。
        if transcript_path(&job.transcript_dir, &id).exists() {
            st.results[i] = skipped_item(&job.transcript_dir, &id);
        } else {
            st.results[i] = match process_one(&client, &http, &id, &job).await {
                Ok(has_seg) => ItemResult {
                    aweme_id: id,
                    state: "succeeded".into(),
                    downloaded: true,
                    transcribed: true,
                    has_segments: has_seg,
                    error: None,
                },
                Err(e) => ItemResult {
                    aweme_id: id,
                    state: "failed".into(),
                    downloaded: false,
                    transcribed: false,
                    has_segments: false,
                    error: Some(e.to_string()),
                },
            };
        }
        crate::events::append(
            task_dir,
            task_id,
            &format!("item.{}", st.results[i].state),
            Some(serde_json::json!({ "aweme_id": st.results[i].aweme_id })),
        );
        recompute_counts(&mut st);
        st.updated_at = now();
        st.heartbeat_at = Some(now());
        write_status(task_dir, &st)?;
    }

    recompute_counts(&mut st);
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
    crate::events::append(
        task_dir,
        task_id,
        &format!("job.{}", st.state),
        Some(serde_json::json!({ "done": st.done, "failed": st.failed, "skipped": st.skipped })),
    );

    // 业务回调：入持久队列并当场尝试投递，通知 zero gateway 触发第二轮 LLM 周期
    // （调 publish_knowledge 回填）。未当场送达则留 pending，由 callback-flush 补发。
    if let Some(handle) = &job.delivery_handle {
        let kind = if st.state == "failed" {
            "douyin-process-failed"
        } else {
            "douyin-process-done"
        };
        let mut payload = serde_json::Map::new();
        payload.insert("task_id".into(), serde_json::json!(st.task_id));
        if let Some(uid) = job.unique_id.as_deref().filter(|s| !s.trim().is_empty()) {
            payload.insert("unique_id".into(), serde_json::json!(uid));
        }
        if let Some(sid) = job.session_id.as_deref().filter(|s| !s.trim().is_empty()) {
            payload.insert("session_id".into(), serde_json::json!(sid));
        }
        match crate::callback::enqueue_and_deliver(
            task_dir,
            &st.task_id,
            kind,
            handle,
            payload,
            crate::callback::GATEWAY_CALLBACK_URL,
        )
        .await
        {
            Ok(true) => {
                st.notified = true;
                st.updated_at = now();
                let _ = write_status(task_dir, &st);
                log::info!("[process callback] notified=true task_id={}", st.task_id);
            }
            Ok(false) => {
                log::warn!(
                    "[process callback] 未当场送达，已入队待 flush task_id={}",
                    st.task_id
                );
            }
            Err(e) => {
                log::warn!(
                    "[process callback] enqueue/deliver failed task_id={}: {e}",
                    st.task_id
                );
            }
        }
    }
    Ok(())
}

/// 下载 mp4 + 通过 `asr-client` 调 FunASR /transcribe + 落 transcript 缓存。返回 has_segments。
async fn process_one(
    client: &DouyinClient,
    http: &reqwest::Client,
    aweme_id: &str,
    job: &Job,
) -> Result<bool> {
    let mp4_path = crate::download::download_one(client, http, aweme_id, &job.out_dir)
        .await
        .context("下载 mp4")?;

    // `job.asr_url` 来自上层（CLI / HTTP submit）的可覆盖参数,形如
    // `http://127.0.0.1:9101/transcribe`。asr-client 的 base 是不带 `/transcribe`
    // 的 host 部分,所以这里去掉尾巴的 `/transcribe`(向后兼容旧 job)。
    let base = job
        .asr_url
        .strip_suffix("/transcribe")
        .unwrap_or(&job.asr_url);
    let asr = asr_client::AsrClient::with_client(http.clone(), base);
    let parsed = asr
        .transcribe_path(
            &mp4_path,
            asr_client::TranscribeOpts { vad: job.vad },
        )
        .await
        .context("调 FunASR /transcribe")?;

    let segments: Vec<Segment> = parsed
        .segments
        .into_iter()
        .map(|s| Segment {
            start: s.t_start,
            end: s.t_end,
            text: s.text,
        })
        .collect();
    let has_segments = !segments.is_empty();

    let transcript = Transcript {
        aweme_id: aweme_id.to_string(),
        text: parsed.text,
        segments,
        has_segments,
        // 用服务端实际回填的模型名(`paraformer` / `sensevoice` / `whisper-*`),
        // 而不是 job 里的占位标签——transcript 能准确记录到底是哪个模型出的活。
        asr_model: parsed.model,
        transcribed_at: now(),
    };
    atomic_write(
        &transcript_path(&job.transcript_dir, aweme_id),
        &serde_json::to_string(&transcript)?,
    )?;
    Ok(has_segments)
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
        heartbeat_at: None,
        notified: false,
    };
    write_status(task_dir, &st)
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

    // /transcribe 响应解析的回归测试现在归属 asr-client crate
    // (`cargo test -p asr-client`)——本 crate 不再持有响应类型。

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
            heartbeat_at: Some(now()),
            notified: false,
        };
        write_status(&dir, &st).unwrap();
        let back = read_status(&dir, "dyproc1").unwrap().unwrap();
        assert_eq!(back.done, 1);
        assert_eq!(back.results.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn running_status(task_id: &str, heartbeat: &str) -> TaskStatus {
        TaskStatus {
            task_id: task_id.into(),
            state: "running".into(),
            total: 1,
            done: 0,
            failed: 0,
            skipped: 0,
            results: vec![],
            updated_at: heartbeat.into(),
            heartbeat_at: Some(heartbeat.into()),
            notified: false,
        }
    }

    #[test]
    fn old_status_without_heartbeat_deserializes() {
        // 旧 status 文件（无 heartbeat_at 字段）应能反序列化，字段退化为 None。
        let raw = r#"{"task_id":"dyproc1","state":"running","total":1,"done":0,
            "failed":0,"skipped":0,"results":[],"updated_at":"2026-05-31T00:00:00Z"}"#;
        let st: TaskStatus = serde_json::from_str(raw).unwrap();
        assert!(st.heartbeat_at.is_none());
    }

    #[test]
    fn is_stale_running_detects_timeout() {
        let now = chrono::Utc::now();
        let old = (now - chrono::Duration::seconds(1000)).to_rfc3339();
        let fresh = (now - chrono::Duration::seconds(10)).to_rfc3339();
        // running + 心跳超时 → stale
        assert!(is_stale_running(&running_status("dyproc1", &old), 600, now));
        // running + 心跳新鲜 → 非 stale
        assert!(!is_stale_running(
            &running_status("dyproc1", &fresh),
            600,
            now
        ));
        // 终态即使心跳很旧也不 reap
        let mut done = running_status("dyproc1", &old);
        done.state = "succeeded".into();
        assert!(!is_stale_running(&done, 600, now));
    }

    #[test]
    fn is_stale_running_falls_back_to_updated_at() {
        let now = chrono::Utc::now();
        let old = (now - chrono::Duration::seconds(1000)).to_rfc3339();
        let mut st = running_status("dyproc1", &old);
        st.heartbeat_at = None; // 旧任务无心跳，退化用 updated_at
        st.updated_at = old;
        assert!(is_stale_running(&st, 600, now));
    }

    #[test]
    fn stale_scan_picks_only_stale_process_running() {
        let dir = tempdir();
        let now = chrono::Utc::now();
        let old = (now - chrono::Duration::seconds(1000)).to_rfc3339();
        let fresh = now.to_rfc3339();
        // stale running process 任务 → 命中
        write_status(&dir, &running_status("dyproc_stale", &old)).unwrap();
        // 新鲜 running → 不命中
        write_status(&dir, &running_status("dyproc_fresh", &fresh)).unwrap();
        // 终态 → 不命中
        let mut done = running_status("dyproc_done", &old);
        done.state = "succeeded".into();
        write_status(&dir, &done).unwrap();
        // 非 dyproc 前缀（如 download 任务）→ 不命中
        write_status(&dir, &running_status("dy_download", &old)).unwrap();

        let mut ids = stale_running_task_ids(&dir, 600).unwrap();
        ids.sort();
        assert_eq!(ids, vec!["dyproc_stale".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retry_missing_job_returns_none() {
        let dir = tempdir();
        assert!(retry(&dir, "nope").unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_ledger_marks_existing_as_skipped() {
        let dir = tempdir();
        // 7a 已有 transcript → skipped；7b 无 → queued
        let t = Transcript {
            aweme_id: "7a".into(),
            text: "x".into(),
            segments: vec![],
            has_segments: false,
            asr_model: "m".into(),
            transcribed_at: now(),
        };
        atomic_write(
            &transcript_path(&dir, "7a"),
            &serde_json::to_string(&t).unwrap(),
        )
        .unwrap();
        let ledger = build_ledger(&dir, &["7a".to_string(), "7b".to_string()]);
        assert_eq!(ledger.len(), 2);
        assert_eq!(ledger[0].state, "skipped");
        assert!(ledger[0].downloaded && ledger[0].transcribed);
        assert_eq!(ledger[1].state, "queued");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recompute_counts_from_ledger() {
        let mut st = TaskStatus {
            task_id: "dyproc1".into(),
            state: "running".into(),
            total: 0,
            done: 0,
            failed: 0,
            skipped: 0,
            results: vec![
                ItemResult {
                    aweme_id: "a".into(),
                    state: "succeeded".into(),
                    downloaded: true,
                    transcribed: true,
                    has_segments: false,
                    error: None,
                },
                ItemResult {
                    aweme_id: "b".into(),
                    state: "skipped".into(),
                    downloaded: true,
                    transcribed: true,
                    has_segments: false,
                    error: None,
                },
                ItemResult {
                    aweme_id: "c".into(),
                    state: "failed".into(),
                    downloaded: false,
                    transcribed: false,
                    has_segments: false,
                    error: Some("e".into()),
                },
                ItemResult {
                    aweme_id: "d".into(),
                    state: "queued".into(),
                    downloaded: false,
                    transcribed: false,
                    has_segments: false,
                    error: None,
                },
            ],
            updated_at: now(),
            heartbeat_at: None,
            notified: false,
        };
        recompute_counts(&mut st);
        assert_eq!(st.total, 4);
        assert_eq!(st.skipped, 1);
        assert_eq!(st.failed, 1);
        assert_eq!(st.done, 2); // succeeded + skipped
    }

    #[test]
    fn submit_prepopulates_ledger() {
        let dir = tempdir();
        let cookie = dir.join("cookie.json");
        let out_dir = dir.join("out");
        let transcript_dir = dir.join("transcripts");
        std::fs::write(&cookie, "{}").unwrap();
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::create_dir_all(&transcript_dir).unwrap();
        // 预置 7a 的 transcript → submit 时账本应标 skipped
        let t = Transcript {
            aweme_id: "7a".into(),
            text: "x".into(),
            segments: vec![],
            has_segments: false,
            asr_model: "m".into(),
            transcribed_at: now(),
        };
        atomic_write(
            &transcript_path(&transcript_dir, "7a"),
            &serde_json::to_string(&t).unwrap(),
        )
        .unwrap();
        let (st, already) = submit(
            &dir,
            &out_dir,
            &transcript_dir,
            &cookie,
            vec!["7a".to_string(), "7b".to_string()],
            "http://127.0.0.1:9101/x".to_string(),
            "sense-voice".to_string(),
            true,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(already, 1);
        assert_eq!(st.results.len(), 2);
        assert_eq!(st.skipped, 1);
        assert_eq!(st.results[0].state, "skipped");
        assert_eq!(st.results[1].state, "queued");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cancel_writes_flag_only_for_active_tasks() {
        let dir = tempdir();
        // 不存在 → false
        assert!(!cancel(&dir, "dyproc_x").unwrap());
        // running → true，落 cancel 标志
        write_status(&dir, &running_status("dyproc_run", &now())).unwrap();
        assert!(cancel(&dir, "dyproc_run").unwrap());
        assert!(is_cancelled(&dir, "dyproc_run"));
        // 终态 → false，不落标志
        let mut done = running_status("dyproc_done", &now());
        done.state = "succeeded".into();
        write_status(&dir, &done).unwrap();
        assert!(!cancel(&dir, "dyproc_done").unwrap());
        assert!(!is_cancelled(&dir, "dyproc_done"));
        // clear 后标志消失
        clear_cancel(&dir, "dyproc_run");
        assert!(!is_cancelled(&dir, "dyproc_run"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retry_requeues_existing_task() {
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
            "http://127.0.0.1:9101/x".to_string(),
            "sense-voice".to_string(),
            true,
            None,
            None,
            None,
        )
        .unwrap();
        // 模拟 worker 崩在 running
        let mut running = read_status(&dir, &st.task_id).unwrap().unwrap();
        running.state = "running".into();
        write_status(&dir, &running).unwrap();
        // retry 应标回 queued（test 下 spawn 不真正起进程）
        let back = retry(&dir, &st.task_id).unwrap().unwrap();
        assert_eq!(back.state, "queued");
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
            "http://127.0.0.1:9101/transcribe".to_string(),
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
