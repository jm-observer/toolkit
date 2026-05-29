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
pub mod download;
pub mod sign;

use anyhow::{Context, Result};
use api::{ApiError, DouyinClient};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn api_error_json(e: &ApiError) -> Value {
    json!({ "error": e.message, "error_kind": e.kind })
}

/// zero 工作区根：来自 `ZERO_WORKSPACE` 环境变量（systemd 注入）。路径不暴露给 LLM，
/// 工具自行派生 cookie / tasks / downloads 子路径（沿用 v1 约定）。
pub fn workspace_dir() -> Result<PathBuf> {
    std::env::var_os("ZERO_WORKSPACE")
        .map(PathBuf::from)
        .context("未显式传路径且 ZERO_WORKSPACE 未设置")
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
    let required = ["msToken", "s_v_web_id", "ttwid"];
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
    let required = ["msToken", "s_v_web_id", "ttwid"];
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

async fn resolve_to_sec_uid(
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
                    json!({
                        "aweme_id": a.get("aweme_id"),
                        "desc": a.get("desc"),
                        "create_time": a.get("create_time"),
                    })
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
}
