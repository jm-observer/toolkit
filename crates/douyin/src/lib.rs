//! zero 的抖音工具库（v2）。
//!
//! 重写自 cv-cat `DouYin_Spider` + jiji262 `douyin-downloader`，纯 Rust 实现签名与 API，
//! 无 Node/JS 依赖。模块：[`sign`] a-bogus 签名、[`api`] web API 客户端、[`download`] 异步下载。
//!
//! 输出契约（与 hf-watcher / github-commit-info 一致）：紧凑 JSON 到 stdout；业务失败输出
//! `{error, error_kind}` 且退出码 0；应用日志走 custom-utils logger（prod 落文件，不污染 stdout）。
//!
//! cookie / 任务目录等路径一律由调用方（zero agent）传绝对路径，工具**不做默认值回退**
//! （与 hf-watcher 的 snapshot_dir 约定一致）。

pub mod api;
pub mod callback;
pub mod client;
pub mod download;
pub mod events;
pub mod knowledge;
pub mod list_works_task;
pub mod process;
pub mod serve;
pub mod sign;

use anyhow::{Context, Result};
use api::{ApiError, DouyinClient};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn api_error_json(e: &ApiError) -> Value {
    json!({ "error": e.message, "error_kind": e.kind })
}

/// zero 工作区根：优先 `ZERO_WORKSPACE` 环境变量；未设置时回退到 `$HOME/.config/zero`
/// （v1 既定布局——zero 服务未注入 ZERO_WORKSPACE，沿用此 fallback）。路径不暴露给 LLM。
pub fn workspace_dir() -> Result<PathBuf> {
    if let Some(ws) = std::env::var_os("ZERO_WORKSPACE") {
        return Ok(PathBuf::from(ws));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("ZERO_WORKSPACE 与 HOME/USERPROFILE 均未设置，无法定位工作区")?;
    Ok(PathBuf::from(home).join(".config").join("zero"))
}

/// cookie 文件：显式路径优先，否则 `$ZERO_WORKSPACE/douyin/cookies.json`。
pub fn resolve_cookie_file(explicit: Option<PathBuf>) -> Result<PathBuf> {
    match explicit {
        Some(p) => Ok(p),
        None => Ok(workspace_dir()?.join("douyin").join("cookies.json")),
    }
}

/// 任务目录：显式优先，否则 `$ZERO_WORKSPACE/douyin/tasks`。
pub fn resolve_task_dir(explicit: Option<PathBuf>) -> Result<PathBuf> {
    match explicit {
        Some(p) => Ok(p),
        None => Ok(workspace_dir()?.join("douyin").join("tasks")),
    }
}

/// 下载输出目录：显式优先，否则 `$ZERO_WORKSPACE/downloads/douyin`。
pub fn resolve_out_dir(explicit: Option<PathBuf>) -> Result<PathBuf> {
    match explicit {
        Some(p) => Ok(p),
        None => Ok(workspace_dir()?.join("downloads").join("douyin")),
    }
}

/// 作品稳定缓存目录（按 unique_id 落 `<id>.json`）：显式优先，否则 `$ZERO_WORKSPACE/douyin/works`。
pub fn resolve_works_dir(explicit: Option<PathBuf>) -> Result<PathBuf> {
    match explicit {
        Some(p) => Ok(p),
        None => Ok(workspace_dir()?.join("douyin").join("works")),
    }
}

/// 知识包根目录（每博主 `<unique_id>/`）：显式优先，否则 `$ZERO_WORKSPACE/knowledge/douyin`。
pub fn resolve_knowledge_dir(explicit: Option<PathBuf>) -> Result<PathBuf> {
    match explicit {
        Some(p) => Ok(p),
        None => Ok(workspace_dir()?.join("knowledge").join("douyin")),
    }
}

/// 转写缓存目录：显式优先，否则 `$ZERO_WORKSPACE/douyin/transcripts`。
pub fn resolve_transcript_dir(explicit: Option<PathBuf>) -> Result<PathBuf> {
    match explicit {
        Some(p) => Ok(p),
        None => Ok(workspace_dir()?.join("douyin").join("transcripts")),
    }
}

/// `list_tags`：聚合某博主已拉取作品的话题标签 + 计数。
pub fn run_list_tags(works_dir: &Path, unique_id: &str) -> Result<Value> {
    knowledge::run_list_tags(works_dir, unique_id)
}

/// `filter_works`：按标签筛选已拉取作品，返回匹配 aweme_ids。
pub fn run_filter_works(
    works_dir: &Path,
    unique_id: &str,
    tags: &[String],
    match_all: bool,
) -> Result<Value> {
    knowledge::run_filter_works(works_dir, unique_id, tags, match_all)
}

/// `publish_knowledge`：把缓存里的作品逐条机械写入知识包目录，有转写缓存则回填。
pub fn run_publish_knowledge(
    works_dir: &Path,
    knowledge_dir: &Path,
    transcript_dir: &Path,
    unique_id: &str,
    only_ids: &[String],
) -> Result<Value> {
    knowledge::run_publish_knowledge(
        works_dir,
        knowledge_dir,
        transcript_dir,
        unique_id,
        only_ids,
    )
}

/// `process_submit`：异步入队「下载+ASR」合并任务，立即返回 task_id。
#[allow(clippy::too_many_arguments)]
pub fn run_process_submit(
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
) -> Result<Value> {
    if ids.is_empty() {
        return Ok(json!({ "error": "ids 为空", "error_kind": "invalid_input" }));
    }
    if let Some(error) = validate_delivery_handle(delivery_handle.as_deref()) {
        return Ok(json!({ "error": error, "error_kind": "invalid_input" }));
    }
    let (st, already) = process::submit(
        task_dir,
        out_dir,
        transcript_dir,
        cookie_file,
        ids,
        asr_url,
        asr_model,
        vad,
        delivery_handle,
        unique_id,
        session_id,
    )?;
    Ok(json!({
        "task_id": st.task_id,
        "submitted": st.total,
        "skipped_already_done": already,
    }))
}

/// `process_status`：查「下载+ASR」任务进度。
pub fn run_process_status(task_dir: &Path, task_id: &str) -> Result<Value> {
    match process::read_status(task_dir, task_id)? {
        Some(st) => Ok(serde_json::to_value(st)?),
        None => Ok(json!({ "error": "任务不存在", "error_kind": "not_found", "task_id": task_id })),
    }
}

/// `process_retry`：重启一个「下载+ASR」任务（重 spawn worker，已完成 item 自动跳过）。
pub fn run_process_retry(task_dir: &Path, task_id: &str) -> Result<Value> {
    match process::retry(task_dir, task_id)? {
        Some(st) => Ok(json!({ "task_id": st.task_id, "state": st.state, "retried": true })),
        None => Ok(json!({ "error": "任务不存在", "error_kind": "not_found", "task_id": task_id })),
    }
}

/// `process_reap`：扫描并重启心跳超时（stale）的 running 任务，返回被 reap 的 task_id。
pub fn run_process_reap(task_dir: &Path, stale_secs: i64) -> Result<Value> {
    let reaped = process::reap(task_dir, stale_secs)?;
    Ok(json!({
        "reaped": reaped.len(),
        "stale_secs": stale_secs,
        "task_ids": reaped,
    }))
}

/// `process_cancel`：请求取消一个「下载+ASR」任务（worker 处理下一条前转 cancelled）。
pub fn run_process_cancel(task_dir: &Path, task_id: &str) -> Result<Value> {
    if process::cancel(task_dir, task_id)? {
        Ok(json!({ "task_id": task_id, "cancel_requested": true }))
    } else {
        Ok(
            json!({ "task_id": task_id, "cancel_requested": false, "error_kind": "not_cancellable" }),
        )
    }
}

/// `list_tasks`：扫描 task_dir，跨三类任务列出精简摘要（task_id/kind/state/时间）。
/// 靠 serde 默认忽略各 status 结构的差异字段，只取公共字段，无需按类型分别解析。
/// `state` 非 None 时按状态过滤。结果按 updated_at 倒序（新任务在前）。
pub fn run_list_tasks(task_dir: &Path, state: Option<&str>) -> Result<Value> {
    #[derive(serde::Deserialize)]
    struct Summary {
        state: String,
        #[serde(default)]
        updated_at: Option<String>,
        #[serde(default)]
        heartbeat_at: Option<String>,
    }
    let mut tasks: Vec<Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(task_dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(task_id) = name.strip_suffix(".status.json") else {
                continue;
            };
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(s) = serde_json::from_str::<Summary>(&raw) else {
                continue;
            };
            if let Some(want) = state {
                if s.state != want {
                    continue;
                }
            }
            let kind = if task_id.starts_with("dyproc") {
                "process"
            } else if task_id.starts_with("dylw") {
                "list-works"
            } else {
                "download"
            };
            tasks.push(json!({
                "task_id": task_id,
                "kind": kind,
                "state": s.state,
                "updated_at": s.updated_at,
                "heartbeat_at": s.heartbeat_at,
            }));
        }
    }
    tasks.sort_by(|a, b| b["updated_at"].as_str().cmp(&a["updated_at"].as_str()));
    Ok(json!({ "count": tasks.len(), "tasks": tasks }))
}

/// `callback_flush`：扫描并补发未送达的持久 callback（pending 且到期的重投一次）。
/// 修掉 §4.4 欠债——worker 当场没送达的通知，由此命令（或定时调用）按退避补发。
pub async fn run_callback_flush(task_dir: &Path) -> Result<Value> {
    let (delivered, pending, failed) =
        callback::flush(task_dir, callback::GATEWAY_CALLBACK_URL).await?;
    Ok(json!({ "delivered": delivered, "pending": pending, "failed": failed }))
}

/// `submit_job`：HTTP `POST /v1/jobs` 的统一入队入口，按 kind 分派到三类 submit。
/// params 缺省路径走 `resolve_*` 默认（与 CLI 一致）。daemon 进程内 spawn worker，
/// worker 脱离 daemon 独立跑。仅 127.0.0.1 可达，信任模型同 CLI。
pub async fn run_submit_job(
    task_dir: &Path,
    kind: &str,
    params: &Value,
    trace_context: Option<String>,
) -> Result<Value> {
    let get_str = |k: &str| params.get(k).and_then(|v| v.as_str()).map(String::from);
    let ids = |k: &str| -> Vec<String> {
        params
            .get(k)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let cookie = resolve_cookie_file(get_str("cookie_file").map(PathBuf::from))?;
    // traceparent 优先取入站 HTTP 头（serve 提取）；缺省时回退 params（nova 发起的
    // 工具调用 zero 设不了头，由 skill 把 traceparent 放进 params 透传）。
    let trace_context = trace_context.or_else(|| get_str("traceparent"));
    let result: Value = match kind {
        "douyin.download" | "download" => {
            let out = resolve_out_dir(get_str("out_dir").map(PathBuf::from))?;
            run_download_submit(&cookie, task_dir, &out, ids("ids")).await?
        }
        "douyin.list-works" | "list-works" => {
            let input = get_str("input").unwrap_or_default();
            let max_pages = params
                .get("max_pages")
                .and_then(|v| v.as_u64())
                .unwrap_or(60) as usize;
            run_list_works_submit(
                &cookie,
                task_dir,
                &input,
                max_pages,
                get_str("delivery_handle").as_deref(),
                get_str("session_id").as_deref(),
            )
            .await?
        }
        "douyin.process" | "process" => {
            let out = resolve_out_dir(get_str("out_dir").map(PathBuf::from))?;
            let tr = resolve_transcript_dir(get_str("transcript_dir").map(PathBuf::from))?;
            let asr_url = get_str("asr_url").unwrap_or_else(|| {
                "http://127.0.0.1:8091/v1/audio/transcriptions/from-source".to_string()
            });
            let asr_model = get_str("asr_model").unwrap_or_else(|| "sense-voice".to_string());
            let vad = params.get("vad").and_then(|v| v.as_bool()).unwrap_or(true);
            run_process_submit(
                task_dir,
                &out,
                &tr,
                &cookie,
                ids("ids"),
                asr_url,
                asr_model,
                vad,
                get_str("delivery_handle"),
                get_str("unique_id"),
                get_str("session_id"),
            )?
        }
        other => {
            return Ok(
                json!({ "error": format!("未知 kind: {other}"), "error_kind": "invalid_input" }),
            );
        }
    };
    // 把提交时捕获的 traceparent 落侧文件，供 worker 终态 enqueue callback 时读入，
    // 实现跨异步（提交→后台下载/处理→完成回调）续接同一条 trace。
    if let (Some(tp), Some(tid)) = (
        trace_context.as_deref(),
        result.get("task_id").and_then(|v| v.as_str()),
    ) {
        if let Err(e) = callback::write_trace(task_dir, tid, tp) {
            log::warn!("[submit] write trace ctx failed: {e}");
        }
    }
    Ok(result)
}

/// `events`：读某任务的 append-only 事件时间线。
pub fn run_events(task_dir: &Path, task_id: &str) -> Result<Value> {
    let evs = events::read_all(task_dir, task_id)?;
    Ok(json!({ "task_id": task_id, "count": evs.len(), "events": evs }))
}

/// 由 task_id 前缀判定任务类型（daemon / HTTP 统一入口按此分派）。
pub fn task_kind(task_id: &str) -> &'static str {
    if task_id.starts_with("dyproc") {
        "process"
    } else if task_id.starts_with("dylw") {
        "list-works"
    } else {
        "download"
    }
}

fn not_found_json(task_id: &str) -> Value {
    json!({ "error": "任务不存在", "error_kind": "not_found", "task_id": task_id })
}

/// 统一查任务状态（按前缀分派到三类）。daemon / HTTP 用。
pub fn run_task_status(task_dir: &Path, task_id: &str) -> Result<Value> {
    let v = match task_kind(task_id) {
        "process" => process::read_status(task_dir, task_id)?
            .map(serde_json::to_value)
            .transpose()?,
        "list-works" => list_works_task::read_status(task_dir, task_id)?
            .map(serde_json::to_value)
            .transpose()?,
        _ => download::read_status(task_dir, task_id)?
            .map(serde_json::to_value)
            .transpose()?,
    };
    Ok(v.unwrap_or_else(|| not_found_json(task_id)))
}

/// 统一重启任务（按前缀分派）。daemon / HTTP 用。
pub fn run_task_retry(task_dir: &Path, task_id: &str) -> Result<Value> {
    let state = match task_kind(task_id) {
        "process" => process::retry(task_dir, task_id)?.map(|s| s.state),
        "list-works" => list_works_task::retry(task_dir, task_id)?.map(|s| s.state),
        _ => download::retry(task_dir, task_id)?.map(|s| s.state),
    };
    match state {
        Some(state) => Ok(json!({ "task_id": task_id, "state": state, "retried": true })),
        None => Ok(not_found_json(task_id)),
    }
}

/// 统一取消任务（按前缀分派）。daemon / HTTP 用。
pub fn run_task_cancel(task_dir: &Path, task_id: &str) -> Result<Value> {
    let ok = match task_kind(task_id) {
        "process" => process::cancel(task_dir, task_id)?,
        "list-works" => list_works_task::cancel(task_dir, task_id)?,
        _ => download::cancel(task_dir, task_id)?,
    };
    if ok {
        Ok(json!({ "task_id": task_id, "cancel_requested": true }))
    } else {
        Ok(
            json!({ "task_id": task_id, "cancel_requested": false, "error_kind": "not_cancellable" }),
        )
    }
}

/// 维护一轮：reap 三类 stale 任务 + flush 未送达 callback。daemon 启动 + 定时调用。
pub async fn run_maintenance(task_dir: &Path, stale_secs: i64) -> Result<Value> {
    let proc_reaped = process::reap(task_dir, stale_secs)?;
    let dl_reaped = download::reap(task_dir, stale_secs)?;
    let lw_reaped = list_works_task::reap(task_dir, stale_secs)?;
    let (delivered, pending, failed) =
        callback::flush(task_dir, callback::GATEWAY_CALLBACK_URL).await?;
    Ok(json!({
        "reaped": {
            "process": proc_reaped,
            "download": dl_reaped,
            "list_works": lw_reaped,
        },
        "callbacks": { "delivered": delivered, "pending": pending, "failed": failed },
    }))
}

/// 读 cookie 文件。支持 v1 结构 `{updated_at, value:{...}}` 或裸 `{...}`。
pub fn load_cookie_file(path: &Path) -> Result<HashMap<String, String>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("读 cookie 文件 {}", path.display()))?;
    let j: Value = serde_json::from_str(&raw).context("解析 cookie JSON")?;
    let obj = j
        .get("value")
        .filter(|v| v.is_object())
        .unwrap_or(&j)
        .as_object()
        .context("cookie JSON 不是对象")?;
    Ok(obj
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect())
}

/// 从 URL / 裸 sec_uid 抽取 sec_uid（不解析短链；短链由调用方先 resolve_redirect）。
fn extract_sec_uid(s: &str) -> Option<String> {
    if let Some(idx) = s.find("/user/") {
        let rest = &s[idx + "/user/".len()..];
        let end = rest.find(['?', '/', '#']).unwrap_or(rest.len());
        let id = &rest[..end];
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    // 裸 sec_uid
    if s.starts_with("MS4w") && !s.contains('/') {
        return Some(s.to_string());
    }
    None
}

/// 是否为需要先跟随重定向的短链。
fn is_short_link(s: &str) -> bool {
    s.contains("v.douyin.com") || s.contains("/share/")
}

/// `cookie_status`：字段自检 + 登录态实测（query/user/）。
pub async fn run_cookie_status(cookie_file: &Path) -> Result<Value> {
    let cookies = load_cookie_file(cookie_file)?;
    // msToken 非必需（实测空值不影响 profile/list/detail，见 api.rs::from_cookies 注释）。
    let required = ["s_v_web_id", "ttwid"];
    let missing: Vec<&str> = required
        .iter()
        .filter(|k| !cookies.contains_key(**k))
        .copied()
        .collect();
    let has_session = cookies.contains_key("sessionid") || cookies.contains_key("sessionid_ss");

    let client = match DouyinClient::from_cookies(&cookies) {
        Ok(c) => c,
        Err(e) => return Ok(api_error_json(&e)),
    };
    let (logged_in, user_uid) = match client.self_info().await {
        Ok(j) => (
            j.get("user_uid").is_some() || j.get("uid").is_some(),
            j.get("user_uid").and_then(|v| v.as_str()).map(String::from),
        ),
        Err(e) => {
            log::warn!("cookie_status self_info failed: {e}");
            (false, None)
        }
    };
    Ok(json!({
        "fields": cookies.len(),
        "has_required": missing.is_empty() && has_session,
        "missing": missing,
        "has_session": has_session,
        "logged_in": logged_in,
        "user_uid": user_uid,
    }))
}

/// `set_cookie`：写 cookies.json（接受浏览器 Cookie 头串或 JSON 对象）。落 v1 结构。
pub async fn run_set_cookie(cookie_file: &Path, raw: &str) -> Result<Value> {
    let trimmed = raw.trim();
    let mut map: HashMap<String, String> = HashMap::new();
    if trimmed.starts_with('{') {
        let j: Value = serde_json::from_str(trimmed).context("解析 cookie JSON 对象")?;
        if let Some(obj) = j.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    map.insert(k.clone(), s.to_string());
                }
            }
        }
    } else {
        for part in trimmed.split("; ") {
            if let Some(eq) = part.find('=') {
                let k = part[..eq].trim();
                let v = &part[eq + 1..];
                if !k.is_empty() {
                    map.insert(k.to_string(), v.to_string());
                }
            }
        }
    }
    // msToken 非必需（实测空值不影响 profile/list/detail，见 api.rs::from_cookies 注释）。
    let required = ["s_v_web_id", "ttwid"];
    let missing: Vec<&str> = required
        .iter()
        .filter(|k| !map.contains_key(**k))
        .copied()
        .collect();
    let out = json!({
        "updated_at": chrono::Utc::now().to_rfc3339(),
        "value": map.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect::<serde_json::Map<_, _>>(),
    });
    if let Some(parent) = cookie_file.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(cookie_file, serde_json::to_string(&out)?)
        .with_context(|| format!("写 cookie 文件 {}", cookie_file.display()))?;
    Ok(json!({
        "written": true,
        "path": cookie_file.to_string_lossy(),
        "fields": map.len(),
        "has_required": missing.is_empty(),
        "missing": missing,
    }))
}

/// `search_user`：v2 降级为 anti_bot-only —— 搜索接口集群被 verify_check 锁，
/// 直接返回引导（让用户改用主页 URL）。仍尝试一次以便上游拿到真实信号。
pub async fn run_search_user(cookie_file: &Path, keyword: &str, count: i64) -> Result<Value> {
    let cookies = load_cookie_file(cookie_file)?;
    let client = match DouyinClient::from_cookies(&cookies) {
        Ok(c) => c,
        Err(e) => return Ok(api_error_json(&e)),
    };
    match client.search_user(keyword, count).await {
        Ok(users) => Ok(json!({ "keyword": keyword, "count": users.len(), "users": users })),
        Err(e) => Ok(api_error_json(&e)),
    }
}

pub(crate) async fn resolve_to_sec_uid(
    client: &DouyinClient,
    input: &str,
) -> Result<Option<String>, ApiError> {
    if let Some(uid) = extract_sec_uid(input) {
        return Ok(Some(uid));
    }
    if input.starts_with("http") && is_short_link(input) {
        let final_url = client.resolve_redirect(input).await?;
        return Ok(extract_sec_uid(&final_url));
    }
    Ok(None)
}

/// `resolve_user`：URL / 短链 / 裸 sec_uid → 博主资料（含 aweme_count）。
pub async fn run_resolve_user(cookie_file: &Path, input: &str) -> Result<Value> {
    let cookies = load_cookie_file(cookie_file)?;
    let client = match DouyinClient::from_cookies(&cookies) {
        Ok(c) => c,
        Err(e) => return Ok(api_error_json(&e)),
    };
    let sec_uid = match resolve_to_sec_uid(&client, input).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return Ok(
                json!({ "error": "无法从输入解析出 sec_uid（仅支持博主主页 URL / 短链 / sec_uid）", "error_kind": "invalid_input" }),
            )
        }
        Err(e) => return Ok(api_error_json(&e)),
    };
    match client.user_profile(&sec_uid).await {
        Ok((nickname, aweme_count, user)) => Ok(json!({
            "sec_uid": sec_uid,
            "nickname": nickname,
            "unique_id": user.get("unique_id"),
            "aweme_count": aweme_count,
            "following_count": user.get("following_count"),
            "follower_count": user.get("follower_count"),
            "signature": user.get("signature"),
        })),
        Err(e) => Ok(api_error_json(&e)),
    }
}

/// `list_works`：列博主作品。throttled 判定 = `has_more 已结束 但 count << aweme_count`
/// （复盘 §6 修正：与每页平均条数无关，是确定性抽稀）。
pub async fn run_list_works(cookie_file: &Path, input: &str, max_pages: usize) -> Result<Value> {
    let cookies = load_cookie_file(cookie_file)?;
    let client = match DouyinClient::from_cookies(&cookies) {
        Ok(c) => c,
        Err(e) => return Ok(api_error_json(&e)),
    };
    let sec_uid = match resolve_to_sec_uid(&client, input).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return Ok(json!({ "error": "无法解析 sec_uid", "error_kind": "invalid_input" }))
        }
        Err(e) => return Ok(api_error_json(&e)),
    };
    let aweme_count = client
        .user_profile(&sec_uid)
        .await
        .map(|(_, c, _)| c)
        .unwrap_or(-1);
    match client.list_all_works(&sec_uid, max_pages).await {
        Ok((works, pages, _)) => {
            let count = works.len() as i64;
            // 抽稀信号：拿到的远少于声明总数（< 90%）。
            let throttled = aweme_count > 0 && count < aweme_count * 9 / 10;
            let items: Vec<Value> = works
                .iter()
                .map(|a| {
                    let ts = a.get("create_time").and_then(|v| v.as_i64()).unwrap_or(0);
                    let ym = chrono::DateTime::from_timestamp(ts, 0)
                        .map(|d| d.format("%Y-%m").to_string())
                        .unwrap_or_default();
                    let mut item = json!({
                        "aweme_id": a.get("aweme_id"),
                        "desc": a.get("desc"),
                        "create_time": a.get("create_time"),
                        "create_ym": ym,
                    });
                    knowledge::enrich_with_tags(&mut item);
                    item
                })
                .collect();
            Ok(json!({
                "sec_uid": sec_uid,
                "aweme_count": aweme_count,
                "count": count,
                "pages_fetched": pages,
                "throttled": throttled,
                "works": items,
            }))
        }
        Err(e) => Ok(api_error_json(&e)),
    }
}

/// `download_submit`：入队下载，立即返回 task_id。
pub async fn run_download_submit(
    cookie_file: &Path,
    task_dir: &Path,
    out_dir: &Path,
    ids: Vec<String>,
) -> Result<Value> {
    if ids.is_empty() {
        return Ok(json!({ "error": "ids 为空", "error_kind": "invalid_input" }));
    }
    let st = download::submit(task_dir, out_dir, cookie_file, ids)?;
    Ok(json!({
        "task_id": st.task_id,
        "state": st.state,
        "total": st.total,
    }))
}

/// `download_status`：查任务进度。
pub async fn run_download_status(task_dir: &Path, task_id: &str) -> Result<Value> {
    match download::read_status(task_dir, task_id)? {
        Some(st) => Ok(serde_json::to_value(st)?),
        None => Ok(json!({ "error": "任务不存在", "error_kind": "not_found", "task_id": task_id })),
    }
}

/// `download_retry`：重启一个下载任务（重 spawn worker，已下载文件靠幂等跳过）。
pub fn run_download_retry(task_dir: &Path, task_id: &str) -> Result<Value> {
    match download::retry(task_dir, task_id)? {
        Some(st) => Ok(json!({ "task_id": st.task_id, "state": st.state, "retried": true })),
        None => Ok(json!({ "error": "任务不存在", "error_kind": "not_found", "task_id": task_id })),
    }
}

/// `download_reap`：扫描并重启心跳超时的 running 下载任务。
pub fn run_download_reap(task_dir: &Path, stale_secs: i64) -> Result<Value> {
    let reaped = download::reap(task_dir, stale_secs)?;
    Ok(json!({ "reaped": reaped.len(), "stale_secs": stale_secs, "task_ids": reaped }))
}

/// `download_cancel`：请求取消一个下载任务（worker 处理下一条前转 cancelled）。
pub fn run_download_cancel(task_dir: &Path, task_id: &str) -> Result<Value> {
    if download::cancel(task_dir, task_id)? {
        Ok(json!({ "task_id": task_id, "cancel_requested": true }))
    } else {
        Ok(
            json!({ "task_id": task_id, "cancel_requested": false, "error_kind": "not_cancellable" }),
        )
    }
}

/// `list_works_submit`：异步入队列博主作品，立即返回 task_id（不阻塞）。
/// `delivery_handle` 透传给 worker，worker 跑完时携带它 POST gateway 触发回调路径
/// （[ADR docs/adr/2026-05-31-callback-driven-async-tasks.md]）；
/// 缺失时 worker 只落 status 不发回调。
pub async fn run_list_works_submit(
    cookie_file: &Path,
    task_dir: &Path,
    input: &str,
    max_pages: usize,
    delivery_handle: Option<&str>,
    session_id: Option<&str>,
) -> Result<Value> {
    if input.trim().is_empty() {
        return Ok(json!({ "error": "input 为空", "error_kind": "invalid_input" }));
    }
    if let Some(error) = validate_delivery_handle(delivery_handle) {
        return Ok(json!({ "error": error, "error_kind": "invalid_input" }));
    }
    let st = list_works_task::submit(
        task_dir,
        cookie_file,
        input.to_string(),
        max_pages,
        delivery_handle.map(str::to_string),
        session_id.map(str::to_string),
    )?;
    Ok(json!({
        "task_id": st.task_id,
        "state": st.state,
        "max_pages": st.max_pages,
    }))
}

fn validate_delivery_handle(handle: Option<&str>) -> Option<String> {
    let handle = handle?;
    let trimmed = handle.trim();
    if trimmed.is_empty() {
        return Some("delivery_handle 为空".to_string());
    }
    if !trimmed.starts_with("dh_") {
        return Some("delivery_handle 格式无效：必须以 dh_ 开头".to_string());
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("placeholder") || lower.contains("demo") {
        return Some("delivery_handle 疑似占位符，拒绝入队".to_string());
    }
    None
}

/// `list_works_status`：查列博主作品任务进度。
/// 终态 succeeded/partial 的 status 含完整 `works[]` + `aweme_count` + `throttled`。
pub async fn run_list_works_status(task_dir: &Path, task_id: &str) -> Result<Value> {
    match list_works_task::read_status(task_dir, task_id)? {
        Some(st) => Ok(serde_json::to_value(st)?),
        None => Ok(json!({ "error": "任务不存在", "error_kind": "not_found", "task_id": task_id })),
    }
}

/// `list_works_retry`：重启一个列作品任务（重 spawn worker，整任务重头翻页）。
pub fn run_list_works_retry(task_dir: &Path, task_id: &str) -> Result<Value> {
    match list_works_task::retry(task_dir, task_id)? {
        Some(st) => Ok(json!({ "task_id": st.task_id, "state": st.state, "retried": true })),
        None => Ok(json!({ "error": "任务不存在", "error_kind": "not_found", "task_id": task_id })),
    }
}

/// `list_works_reap`：扫描并重启心跳超时的 running 列作品任务。
pub fn run_list_works_reap(task_dir: &Path, stale_secs: i64) -> Result<Value> {
    let reaped = list_works_task::reap(task_dir, stale_secs)?;
    Ok(json!({ "reaped": reaped.len(), "stale_secs": stale_secs, "task_ids": reaped }))
}

/// `list_works_cancel`：请求取消一个列作品任务（worker 翻下一页前转 cancelled）。
pub fn run_list_works_cancel(task_dir: &Path, task_id: &str) -> Result<Value> {
    if list_works_task::cancel(task_dir, task_id)? {
        Ok(json!({ "task_id": task_id, "cancel_requested": true }))
    } else {
        Ok(
            json!({ "task_id": task_id, "cancel_requested": false, "error_kind": "not_cancellable" }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_sec_uid_from_url() {
        assert_eq!(
            extract_sec_uid("https://www.douyin.com/user/MS4wLjABAAAAabc123?foo=1"),
            Some("MS4wLjABAAAAabc123".to_string())
        );
    }

    #[test]
    fn extract_sec_uid_bare() {
        assert_eq!(
            extract_sec_uid("MS4wLjABAAAAxyz"),
            Some("MS4wLjABAAAAxyz".to_string())
        );
    }

    #[test]
    fn extract_sec_uid_none() {
        assert_eq!(extract_sec_uid("https://www.douyin.com/video/12345"), None);
    }

    #[test]
    fn short_link_detection() {
        assert!(is_short_link("https://v.douyin.com/abc/"));
        assert!(!is_short_link("https://www.douyin.com/user/MS4w"));
    }

    #[test]
    fn delivery_handle_validation_accepts_real_handle() {
        assert!(validate_delivery_handle(Some("dh_8a2f4c91")).is_none());
        assert!(validate_delivery_handle(None).is_none());
    }

    #[test]
    fn delivery_handle_validation_rejects_placeholder() {
        assert!(validate_delivery_handle(Some("dh_placeholder_for_demo")).is_some());
        assert!(validate_delivery_handle(Some("placeholder")).is_some());
        assert!(validate_delivery_handle(Some("abc")).is_some());
    }
}
