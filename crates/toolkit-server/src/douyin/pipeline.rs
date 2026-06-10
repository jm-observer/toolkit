//! Phase 2：`douyin_pipeline` 编排 TaskKind —— 串联抖音知识管线全链路。
//!
//! 输入：unique_id 或主页 URL + 可选标签筛选 + 各环节开关。
//! 串联：list_works(可选) → download → ASR(process) → text_refine → kb_publish → rag ingest。
//!
//! 每个下游环节复用既有「submit → 轮询下游文件状态 → 终态」模式（与 kinds.rs 同款），
//! 整体进度聚合写 progress：`{stage, stage_index, stage_total, stage_progress, done, total}`。
//! 任一环节失败 → 任务 failed，已完成环节的成果保留（下游任务自身幂等：已完成 item 自动
//! 跳过 —— download/process 看 transcript 缓存账本、text_refine 看 refined 缓存、kb_publish
//! 内容确定重跑覆盖）。重跑整条 pipeline 安全。

use crate::douyin_mod::kinds::{extract_task_id, read_status, DouyinKind};
use crate::douyin_mod::paths::DouyinPaths;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use toolkit_tasks::{TaskCtx, TaskKind};

const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Serialize, Deserialize)]
pub struct PipelineInput {
    /// 抖音号（unique_id）或主页 URL / 短链 / sec_uid。
    pub handle: String,
    /// 标签筛选；为空表示不筛选（处理全部已缓存作品）。
    #[serde(default)]
    pub tags: Vec<String>,
    /// 标签匹配：true=须含全部标签，false=含任一。
    #[serde(default)]
    pub match_all: bool,
    /// list_works 翻页上限（仅 `sync_works` 开启时用）。
    #[serde(default = "default_max_pages")]
    pub max_pages: usize,
    /// 环节开关。
    #[serde(default)]
    pub stages: StageToggles,
    /// ASR 端点（缺省走本机 asr-server from-source）。
    #[serde(default)]
    pub asr_url: Option<String>,
    #[serde(default)]
    pub asr_model: Option<String>,
    /// rag ingest 用的 rag 配置 JSON 绝对路径（开启 `rag_ingest` 时必填）。
    #[serde(default)]
    pub rag_config: Option<String>,
}

fn default_max_pages() -> usize {
    60
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StageToggles {
    /// 先拉一次作品列表（刷新作品缓存）。默认 false：依赖已有缓存。
    #[serde(default)]
    pub sync_works: bool,
    #[serde(default = "default_true")]
    pub download: bool,
    #[serde(default = "default_true")]
    pub transcribe: bool,
    #[serde(default = "default_true")]
    pub refine: bool,
    #[serde(default = "default_true")]
    pub kb_publish: bool,
    /// rag 录入。默认 false（需 rag_config）。
    #[serde(default)]
    pub rag_ingest: bool,
}

impl Default for StageToggles {
    fn default() -> Self {
        Self {
            sync_works: false,
            download: true,
            transcribe: true,
            refine: true,
            kb_publish: true,
            rag_ingest: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PipelineOutput {
    pub unique_id: String,
    pub aweme_ids: Vec<String>,
    pub stages: Vec<StageResult>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StageResult {
    pub stage: String,
    pub ok: bool,
    pub detail: Value,
}

pub struct DouyinPipeline;

#[async_trait]
impl TaskKind for DouyinPipeline {
    type Input = PipelineInput;
    type Output = PipelineOutput;
    const KIND: &'static str = "douyin_pipeline";

    async fn run(input: PipelineInput, ctx: TaskCtx) -> Result<PipelineOutput> {
        if input.handle.trim().is_empty() {
            bail!("handle 为空");
        }
        // refine 环节需要 LLM 配置：提早校验，避免跑完下载/ASR 才在 refine 报配置缺失。
        if input.stages.refine {
            douyin::refine::LlmConfig::from_env().context("pipeline 开启 refine 但 LLM 未配置")?;
        }
        if input.stages.rag_ingest && input.rag_config.is_none() {
            bail!("pipeline 开启 rag_ingest 但未提供 rag_config 路径");
        }

        let paths = DouyinPaths::new(&ctx.data_dir);
        paths.ensure_dirs()?;

        // 计算总阶段数（用于进度聚合）。
        let mut stage_names: Vec<&str> = Vec::new();
        if input.stages.sync_works {
            stage_names.push("sync_works");
        }
        stage_names.push("resolve"); // 解析 unique_id + 选 aweme_ids
        if input.stages.download {
            stage_names.push("download");
        }
        if input.stages.transcribe {
            stage_names.push("transcribe");
        }
        if input.stages.refine {
            stage_names.push("refine");
        }
        if input.stages.kb_publish {
            stage_names.push("kb_publish");
        }
        if input.stages.rag_ingest {
            stage_names.push("rag_ingest");
        }
        let stage_total = stage_names.len();
        let mut stage_index = 0usize;
        let mut results: Vec<StageResult> = Vec::new();

        let report =
            |ctx: &TaskCtx, idx: usize, stage: &str, stage_progress: Value| -> Result<()> {
                ctx.report_progress(json!({
                    "stage": stage,
                    "stage_index": idx,
                    "stage_total": stage_total,
                    "stage_progress": stage_progress,
                }))
            };

        // ---------- sync_works（可选） ----------
        if input.stages.sync_works {
            stage_index += 1;
            report(
                &ctx,
                stage_index,
                "sync_works",
                json!({"state": "submitted"}),
            )?;
            let submit = douyin::run_list_works_submit(
                &paths.cookie_file,
                &paths.task_dir,
                &input.handle,
                input.max_pages,
                None,
                None,
            )
            .await?;
            let dy = extract_task_id(&submit, "list_works")?;
            let status = poll_stage(
                &ctx,
                &paths,
                &dy,
                DouyinKind::ListWorks,
                stage_index,
                "sync_works",
            )
            .await?;
            results.push(StageResult {
                stage: "sync_works".into(),
                ok: true,
                detail: status,
            });
        }

        // ---------- resolve：unique_id + 选 aweme_ids ----------
        stage_index += 1;
        report(&ctx, stage_index, "resolve", json!({"state": "resolving"}))?;
        let unique_id = resolve_unique_id(&paths, &input.handle).await?;
        let aweme_ids = select_aweme_ids(&paths, &unique_id, &input.tags, input.match_all)?;
        if aweme_ids.is_empty() {
            bail!("未选出任何 aweme_id（unique_id={unique_id}）：请确认已 list_works 且标签筛选有命中");
        }
        results.push(StageResult {
            stage: "resolve".into(),
            ok: true,
            detail: json!({ "unique_id": unique_id, "selected": aweme_ids.len() }),
        });

        // ---------- download ----------
        if input.stages.download {
            stage_index += 1;
            report(&ctx, stage_index, "download", json!({"state": "submitted"}))?;
            let submit = douyin::run_download_submit(
                &paths.cookie_file,
                &paths.task_dir,
                &paths.out_dir,
                aweme_ids.clone(),
            )
            .await?;
            let dy = extract_task_id(&submit, "download")?;
            let status = poll_stage(
                &ctx,
                &paths,
                &dy,
                DouyinKind::Download,
                stage_index,
                "download",
            )
            .await?;
            results.push(StageResult {
                stage: "download".into(),
                ok: true,
                detail: status,
            });
        }

        // ---------- transcribe（ASR，process 任务同时下载+转写，幂等跳过已下载） ----------
        if input.stages.transcribe {
            stage_index += 1;
            report(
                &ctx,
                stage_index,
                "transcribe",
                json!({"state": "submitted"}),
            )?;
            let asr_url = input.asr_url.clone().unwrap_or_else(|| {
                "http://127.0.0.1:8091/v1/audio/transcriptions/from-source".to_string()
            });
            let asr_model = input
                .asr_model
                .clone()
                .unwrap_or_else(|| "sense-voice".to_string());
            let submit = douyin::run_process_submit(
                &paths.task_dir,
                &paths.out_dir,
                &paths.transcript_dir,
                &paths.cookie_file,
                aweme_ids.clone(),
                asr_url,
                asr_model,
                true,
                None,
                Some(unique_id.clone()),
                None,
            )?;
            let dy = extract_task_id(&submit, "process")?;
            let status = poll_stage(
                &ctx,
                &paths,
                &dy,
                DouyinKind::Process,
                stage_index,
                "transcribe",
            )
            .await?;
            results.push(StageResult {
                stage: "transcribe".into(),
                ok: true,
                detail: status,
            });
        }

        // ---------- refine（直接进程内逐条调 LLM，幂等跳过已整理） ----------
        if input.stages.refine {
            stage_index += 1;
            report(&ctx, stage_index, "refine", json!({"state": "running"}))?;
            let detail = run_refine_inline(&ctx, &paths, &aweme_ids, stage_index, stage_total)
                .await
                .context("refine 环节失败")?;
            results.push(StageResult {
                stage: "refine".into(),
                ok: true,
                detail,
            });
        }

        // ---------- kb_publish（同步，内容确定，重跑覆盖） ----------
        if input.stages.kb_publish {
            stage_index += 1;
            report(&ctx, stage_index, "kb_publish", json!({"state": "running"}))?;
            let v = douyin::run_publish_knowledge(
                &paths.works_dir,
                &paths.knowledge_dir,
                &paths.transcript_dir,
                &paths.refined_dir,
                &unique_id,
                &aweme_ids,
            )
            .context("kb_publish 失败")?;
            if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
                bail!("kb_publish 返回错误: {err}");
            }
            results.push(StageResult {
                stage: "kb_publish".into(),
                ok: true,
                detail: v,
            });
        }

        // ---------- rag_ingest（调 rag 二进制全量扫描入向量库，upsert 幂等） ----------
        if input.stages.rag_ingest {
            stage_index += 1;
            report(&ctx, stage_index, "rag_ingest", json!({"state": "running"}))?;
            let cfg = input
                .rag_config
                .clone()
                .ok_or_else(|| anyhow!("缺 rag_config"))?;
            let v = run_rag_ingest(&ctx.data_dir, &cfg)
                .await
                .context("rag ingest 失败")?;
            results.push(StageResult {
                stage: "rag_ingest".into(),
                ok: true,
                detail: v,
            });
        }

        report(&ctx, stage_total, "done", json!({"state": "done"}))?;
        Ok(PipelineOutput {
            unique_id,
            aweme_ids,
            stages: results,
        })
    }
}

/// 解析 unique_id：输入已是裸 unique_id（无 http / 无斜杠 / 有作品缓存）则直接用，
/// 否则调 resolve_user 解析 URL/短链/sec_uid。
async fn resolve_unique_id(paths: &DouyinPaths, handle: &str) -> Result<String> {
    let h = handle.trim();
    // 已有该 handle 的作品缓存 → 直接当 unique_id 用（避免无谓网络调用）。
    if paths.works_dir.join(format!("{h}.json")).exists() {
        return Ok(h.to_string());
    }
    let v = douyin::run_resolve_user(&paths.cookie_file, h).await?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        bail!("resolve_user 失败: {err}");
    }
    let uid = v
        .get("unique_id")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| anyhow!("resolve_user 未返回 unique_id"))?;
    Ok(uid)
}

/// 选 aweme_ids：有标签则 filter_works，否则取该博主缓存里全部作品。
fn select_aweme_ids(
    paths: &DouyinPaths,
    unique_id: &str,
    tags: &[String],
    match_all: bool,
) -> Result<Vec<String>> {
    if tags.is_empty() {
        // 空标签 = 全部：用 any-match 的 filter 不合适，直接读缓存全量。
        let v = douyin::run_list_tags(&paths.works_dir, unique_id)?;
        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            bail!("{err}");
        }
        // list_tags 不直接给 aweme_ids；改用 filter_works 取全部需要标签——
        // 退而用 filter 的「任一标签」对全标签集合不实际，故这里读作品缓存全量。
        return read_all_aweme_ids(paths, unique_id);
    }
    let v = douyin::run_filter_works(&paths.works_dir, unique_id, tags, match_all)?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        bail!("filter_works 失败: {err}");
    }
    let ids = v
        .get("aweme_ids")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(ids)
}

/// 读某博主作品缓存里全部 aweme_id（filter_works 对全集合不便时的全量路径）。
fn read_all_aweme_ids(paths: &DouyinPaths, unique_id: &str) -> Result<Vec<String>> {
    let p = paths.works_dir.join(format!("{unique_id}.json"));
    let raw = std::fs::read_to_string(&p)
        .with_context(|| format!("读作品缓存 {}（请先 list_works）", p.display()))?;
    let cache: Value = serde_json::from_str(&raw).context("解析作品缓存")?;
    let ids = cache
        .get("works")
        .and_then(|w| w.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|w| w.get("aweme_id").and_then(|x| x.as_str()).map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(ids)
}

/// 轮询一个下游文件型任务到终态，把状态聚合进 pipeline progress。
async fn poll_stage(
    ctx: &TaskCtx,
    paths: &DouyinPaths,
    dy_task_id: &str,
    kind: DouyinKind,
    stage_index: usize,
    stage: &str,
) -> Result<Value> {
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let status = read_status(paths, dy_task_id, kind).await?;
        if let Some(err) = status.get("error").and_then(|v| v.as_str()) {
            bail!("{stage} 下游错误: {err}");
        }
        let state = status
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        ctx.report_progress(json!({
            "stage": stage,
            "stage_index": stage_index,
            "stage_progress": status.clone(),
            "douyin_task_id": dy_task_id,
        }))?;
        match state.as_str() {
            "queued" | "running" => continue,
            "succeeded" | "partial" => return Ok(status),
            "failed" => bail!(
                "{stage} 失败: {}",
                status
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)")
            ),
            "cancelled" => bail!("{stage} 被取消"),
            other => bail!("{stage} 未知状态: {other}"),
        }
    }
}

/// pipeline 内联整理：逐条调 LLM，复用 douyin::refine。失败累计但不中断（与独立
/// TextRefine 任务语义一致）。返回统计 detail。
async fn run_refine_inline(
    ctx: &TaskCtx,
    paths: &DouyinPaths,
    aweme_ids: &[String],
    stage_index: usize,
    stage_total: usize,
) -> Result<Value> {
    let cfg = douyin::refine::LlmConfig::from_env()?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()?;
    let mut refined = 0usize;
    let mut skipped = 0usize;
    let mut failures: Vec<Value> = Vec::new();
    let total = aweme_ids.len();
    for (i, id) in aweme_ids.iter().enumerate() {
        // 幂等：已整理则跳过。
        if douyin::refine::read_refined(&paths.refined_dir, id).is_some() {
            skipped += 1;
        } else {
            match douyin::process::read_transcript(&paths.transcript_dir, id) {
                Some(t) if !t.text.trim().is_empty() => {
                    match douyin::refine::refine_one(&http, &cfg, &paths.refined_dir, id, &t.text)
                        .await
                    {
                        Ok(_) => refined += 1,
                        Err(e) => failures.push(json!({"aweme_id": id, "error": format!("{e:#}")})),
                    }
                }
                _ => skipped += 1, // 无转写或空文本：跳过（download/transcribe 关时正常）
            }
        }
        ctx.report_progress(json!({
            "stage": "refine",
            "stage_index": stage_index,
            "stage_total": stage_total,
            "stage_progress": { "total": total, "done": i + 1, "refined": refined, "skipped": skipped, "failed": failures.len() },
        }))?;
    }
    Ok(json!({
        "total": total,
        "refined": refined,
        "skipped": skipped,
        "failed": failures.len(),
        "failures": failures,
    }))
}

/// 调 `rag` 二进制做全量 ingest（与 rag CLI 契约一致：stdout 一行 JSON）。
/// 选用「调命令」而非「lib 直连」：rag ingest 需 embedding HTTP + sqlite-vec store，
/// 这些重依赖不必拉进 toolkit-server；rag 二进制与 toolkit-server 同机部署。
async fn run_rag_ingest(workspace: &std::path::Path, rag_config: &str) -> Result<Value> {
    let exe = which_rag()?;
    let output = tokio::process::Command::new(&exe)
        .arg("ingest")
        .arg("--config")
        .arg(rag_config)
        .arg("--workspace")
        .arg(workspace)
        .output()
        .await
        .with_context(|| format!("spawn rag ingest（{}）", exe.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let last = stdout.lines().last().unwrap_or("").trim();
    if last.is_empty() {
        bail!(
            "rag ingest 无输出（stderr: {}）",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(300)
                .collect::<String>()
        );
    }
    let v: Value = serde_json::from_str(last).with_context(|| format!("解析 rag 输出: {last}"))?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        bail!("rag ingest 失败: {err}");
    }
    Ok(v)
}

/// 定位 rag 二进制：优先环境变量 `RAG_BIN`，否则取与当前可执行同目录下的 `rag`。
fn which_rag() -> Result<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("RAG_BIN") {
        return Ok(std::path::PathBuf::from(p));
    }
    let exe = std::env::current_exe().context("取当前可执行路径")?;
    let dir = exe.parent().ok_or_else(|| anyhow!("无法定位可执行目录"))?;
    let candidate = dir.join(if cfg!(windows) { "rag.exe" } else { "rag" });
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        let id = C.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("pipeline-test-{}-{}", std::process::id(), id));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// 写一份 douyin works 缓存 fixture（与 list_works worker 落盘结构一致）。
    fn write_works_cache(paths: &DouyinPaths, unique_id: &str) {
        std::fs::create_dir_all(&paths.works_dir).unwrap();
        let cache = json!({
            "sec_uid": "MS4wTEST",
            "unique_id": unique_id,
            "nickname": "测试博主",
            "aweme_count": 3,
            "count": 3,
            "throttled": false,
            "cached_at": "2026-06-10T00:00:00Z",
            "works": [
                {"aweme_id":"a1","desc":"入门 #数字 #英语","create_ym":"2026-05","tags":["数字","英语"]},
                {"aweme_id":"a2","desc":"进阶 #数字","create_ym":"2026-04","tags":["数字"]},
                {"aweme_id":"a3","desc":"杂谈 #日常","create_ym":"2026-03","tags":["日常"]}
            ]
        });
        std::fs::write(
            paths.works_dir.join(format!("{unique_id}.json")),
            cache.to_string(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn resolve_unique_id_uses_cache_without_network() {
        let dir = tempdir();
        let paths = DouyinPaths::new(&dir);
        write_works_cache(&paths, "82933463317");
        // 缓存命中 → 直接当 unique_id 用，不触网（无 cookie 也不报错）。
        let uid = resolve_unique_id(&paths, "82933463317").await.unwrap();
        assert_eq!(uid, "82933463317");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn select_all_when_no_tags() {
        let dir = tempdir();
        let paths = DouyinPaths::new(&dir);
        write_works_cache(&paths, "uid1");
        let ids = select_aweme_ids(&paths, "uid1", &[], false).unwrap();
        assert_eq!(ids, vec!["a1", "a2", "a3"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn select_filtered_by_tag() {
        let dir = tempdir();
        let paths = DouyinPaths::new(&dir);
        write_works_cache(&paths, "uid1");
        // 「数字」标签命中 a1 + a2。
        let mut ids = select_aweme_ids(&paths, "uid1", &["数字".to_string()], false).unwrap();
        ids.sort();
        assert_eq!(ids, vec!["a1", "a2"]);
        // match_all：「数字」+「英语」仅 a1。
        let all = select_aweme_ids(
            &paths,
            "uid1",
            &["数字".to_string(), "英语".to_string()],
            true,
        )
        .unwrap();
        assert_eq!(all, vec!["a1"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn select_missing_cache_errors() {
        let dir = tempdir();
        let paths = DouyinPaths::new(&dir);
        // 无缓存 → list_tags 返回 not_listed → select 报错。
        let err = select_aweme_ids(&paths, "nope", &[], false).unwrap_err();
        assert!(format!("{err:#}").contains("缓存") || format!("{err:#}").contains("list"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
