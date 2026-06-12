//! 3 个抖音 TaskKind：包装 douyin crate 已有的 submit + 文件状态轮询。
//!
//! 形态统一：
//! 1. `run` 内部调 `douyin::run_*_submit` → 拿 `douyin_task_id`
//! 2. 上报一次 progress（含 douyin_task_id），让外部能在 toolkit tasks 表看到下游 ID
//! 3. 每 2 秒读 douyin 状态，把整张状态写进 progress
//! 4. terminal succeeded/partial → `run` 返回 Ok(完整 douyin 状态)；failed/cancelled → Err

use crate::douyin_mod::paths::DouyinPaths;
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use toolkit_tasks::{Registry, TaskCtx, TaskKind};

const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// 注册所有抖音 kind。
pub fn register_all(reg: &mut Registry) {
    reg.register::<DouyinDownload>();
    reg.register::<DouyinTranscribe>();
    reg.register::<DouyinListWorks>();
    reg.register::<super::refine::DouyinTextRefine>();
    reg.register::<super::pipeline::DouyinPipeline>();
}

// ---------- DouyinDownload ----------

#[derive(Debug, Serialize, Deserialize)]
pub struct DownloadInput {
    /// 抖音 aweme_id 列表。
    pub aweme_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LongTaskOutput {
    pub douyin_task_id: String,
    pub status: Value,
}

pub struct DouyinDownload;

#[async_trait]
impl TaskKind for DouyinDownload {
    type Input = DownloadInput;
    type Output = LongTaskOutput;
    const KIND: &'static str = "douyin_download";

    async fn run(input: DownloadInput, ctx: TaskCtx) -> Result<LongTaskOutput> {
        if input.aweme_ids.is_empty() {
            bail!("aweme_ids 为空");
        }
        let paths = DouyinPaths::new(&ctx.data_dir);
        paths.ensure_dirs()?;
        let submit = douyin::run_download_submit(
            &paths.cookie_file,
            &paths.task_dir,
            &paths.out_dir,
            input.aweme_ids,
        )
        .await?;
        let dy_task_id = extract_task_id(&submit, "download")?;
        ctx.report_progress(json!({"douyin_task_id": dy_task_id, "stage": "submitted"}))?;
        poll_until_terminal(&ctx, &paths, &dy_task_id, DouyinKind::Download).await
    }
}

// ---------- DouyinTranscribe ----------

#[derive(Debug, Serialize, Deserialize)]
pub struct TranscribeInput {
    pub aweme_ids: Vec<String>,
    #[serde(default = "default_vad")]
    pub vad: bool,
    #[serde(default)]
    pub asr_url: Option<String>,
    #[serde(default)]
    pub asr_model: Option<String>,
    #[serde(default)]
    pub unique_id: Option<String>,
}

fn default_vad() -> bool {
    true
}

pub struct DouyinTranscribe;

#[async_trait]
impl TaskKind for DouyinTranscribe {
    type Input = TranscribeInput;
    type Output = LongTaskOutput;
    const KIND: &'static str = "douyin_transcribe";

    async fn run(input: TranscribeInput, ctx: TaskCtx) -> Result<LongTaskOutput> {
        if input.aweme_ids.is_empty() {
            bail!("aweme_ids 为空");
        }
        let paths = DouyinPaths::new(&ctx.data_dir);
        paths.ensure_dirs()?;
        let asr_url = input
            .asr_url
            .unwrap_or_else(|| "http://127.0.0.1:9101/transcribe".to_string());
        let asr_model = input.asr_model.unwrap_or_else(|| "funasr".to_string());
        let submit = douyin::run_process_submit(
            &paths.task_dir,
            &paths.out_dir,
            &paths.transcript_dir,
            &paths.cookie_file,
            input.aweme_ids,
            asr_url,
            asr_model,
            input.vad,
            None,
            input.unique_id,
            None,
        )?;
        let dy_task_id = extract_task_id(&submit, "process")?;
        ctx.report_progress(json!({"douyin_task_id": dy_task_id, "stage": "submitted"}))?;
        poll_until_terminal(&ctx, &paths, &dy_task_id, DouyinKind::Process).await
    }
}

// ---------- DouyinListWorks ----------

#[derive(Debug, Serialize, Deserialize)]
pub struct ListWorksInput {
    pub handle: String,
    #[serde(default = "default_max_pages")]
    pub max_pages: usize,
}

fn default_max_pages() -> usize {
    60
}

pub struct DouyinListWorks;

#[async_trait]
impl TaskKind for DouyinListWorks {
    type Input = ListWorksInput;
    type Output = LongTaskOutput;
    const KIND: &'static str = "douyin_list_works";

    async fn run(input: ListWorksInput, ctx: TaskCtx) -> Result<LongTaskOutput> {
        if input.handle.trim().is_empty() {
            bail!("handle 为空");
        }
        let paths = DouyinPaths::new(&ctx.data_dir);
        paths.ensure_dirs()?;
        let submit = douyin::run_list_works_submit(
            &paths.cookie_file,
            &paths.task_dir,
            &input.handle,
            input.max_pages,
            None,
            None,
        )
        .await?;
        let dy_task_id = extract_task_id(&submit, "list_works")?;
        ctx.report_progress(json!({"douyin_task_id": dy_task_id, "stage": "submitted"}))?;
        poll_until_terminal(&ctx, &paths, &dy_task_id, DouyinKind::ListWorks).await
    }
}

// ---------- 共用工具 ----------

#[derive(Copy, Clone)]
pub(crate) enum DouyinKind {
    Download,
    Process,
    ListWorks,
}

pub(crate) fn extract_task_id(submit_result: &Value, label: &str) -> Result<String> {
    if let Some(err) = submit_result.get("error").and_then(|v| v.as_str()) {
        bail!("douyin {label} submit failed: {err}");
    }
    submit_result
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow!("douyin {label} submit response missing task_id"))
}

async fn poll_until_terminal(
    ctx: &TaskCtx,
    paths: &DouyinPaths,
    dy_task_id: &str,
    kind: DouyinKind,
) -> Result<LongTaskOutput> {
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let status = read_status(paths, dy_task_id, kind).await?;
        if let Some(err) = status.get("error").and_then(|v| v.as_str()) {
            bail!("douyin status returned error: {err}");
        }
        let state = status
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        ctx.report_progress(json!({
            "douyin_task_id": dy_task_id,
            "state": state,
            "raw": status.clone(),
        }))?;
        match state.as_str() {
            "queued" | "running" => continue,
            "succeeded" | "partial" => {
                return Ok(LongTaskOutput {
                    douyin_task_id: dy_task_id.to_string(),
                    status,
                });
            }
            "failed" => {
                let err = status
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)")
                    .to_string();
                bail!("douyin task failed: {err}");
            }
            "cancelled" => bail!("douyin task cancelled"),
            other => bail!("douyin task unknown state: {other}"),
        }
    }
}

pub(crate) async fn read_status(
    paths: &DouyinPaths,
    dy_task_id: &str,
    kind: DouyinKind,
) -> Result<Value> {
    match kind {
        DouyinKind::Download => douyin::run_download_status(&paths.task_dir, dy_task_id).await,
        DouyinKind::Process => Ok(douyin::run_process_status(&paths.task_dir, dy_task_id)?),
        DouyinKind::ListWorks => douyin::run_list_works_status(&paths.task_dir, dy_task_id).await,
    }
}
