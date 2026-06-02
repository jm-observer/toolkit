//! 异步列博主作品：`submit` 立返 task_id；后台 worker 进程翻页 + 增量 status；
//! `status` 读状态文件。绕开 nova 30s 硬超时与"模型看见 timeout 反复重试"的反模式。
//!
//! 状态机：queued → running → (succeeded | partial | failed)
//! - succeeded：worker 跑完且 throttled=false 或 count >= aweme_count
//! - partial：worker 跑完但 throttled=true 且 count < aweme_count（出口 IP 被抖音抽稀）
//! - failed：cookie 不可用 / sec_uid 解析失败 / 翻页第一页就 ApiError
//!
//! 与 download.rs 同款 fire-and-forget 进程模型；helper 函数（atomic_write 等）就近
//! 复制一份（30 行成本 vs 抽公共模块的开销，权衡选前者；详见 RFC §2 风险项）。

use crate::api::DouyinClient;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 列作品任务状态（落盘 JSON，原子替换）。
///
/// `works` 字段在 running 阶段保持 `[]` 节省 status 文件；只在终态填完整列表。
/// 字段语义与 `run_list_works`（同步版）对齐，子 Agent 解析逻辑可复用。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaskStatus {
    pub task_id: String,
    pub state: String,
    pub sec_uid: Option<String>,
    /// 博主昵称，worker resolve_user 后写入；让回调路径不必再调 resolve_user
    /// 拿名字（避免一次 30s API 调用）。
    #[serde(default)]
    pub nickname: Option<String>,
    /// 抖音号（unique_id），worker user_profile 后写入。知识包目录键用它
    /// （Plan 5：`knowledge/douyin/<unique_id>/`）。
    #[serde(default)]
    pub unique_id: Option<String>,
    pub pages_fetched: usize,
    pub max_pages: usize,
    pub aweme_count: i64,
    pub count: usize,
    pub throttled: bool,
    pub works: Vec<Value>,
    pub error: Option<String>,
    pub updated_at: String,
    /// worker 存活证明：running 期间每页刷新。reap 据此判 stale（见 process.rs 同款）。
    #[serde(default)]
    pub heartbeat_at: Option<String>,
    /// 是否已经成功 POST gateway 通知 zero。`false` = 业务回调通道还没成功
    /// （worker 未到 POST 步、或 POST 3 次都失败），`true` = 已成功通知。
    /// alarm 兜底子 Agent 据此判定"补救下发"还是"静默退出"。
    #[serde(default)]
    pub notified: bool,
}

/// 任务作业描述（submit 写、worker 读）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Job {
    pub task_id: String,
    pub handle: String,
    pub max_pages: usize,
    pub cookie_file: PathBuf,
    /// delivery_handle（dh_xxx）—— worker 跑完 POST gateway 时携带，让回包能
    /// 投递回原发起者（与 alarm 老路径同款）。`None` 时 worker 仍跑完业务，
    /// 但不发回调（适合 CLI 手动 submit 测试场景）。
    #[serde(default)]
    pub delivery_handle: Option<String>,
    /// E2E 测试用 zero session_id。worker POST callback 时回填到 payload，
    /// sps correlate 可据此关联 sub-agent LLM 调用。生产场景传 None 不影响功能。
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

/// 原子写：先写 .tmp 再 rename，避免 status 读到半截 JSON。
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

/// 读 task_id 的当前 status；不存在返回 None（status CLI 据此报 task_not_found）。
pub fn read_status(task_dir: &Path, task_id: &str) -> Result<Option<TaskStatus>> {
    let p = status_path(task_dir, task_id);
    if !p.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&p)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

/// 入队：生成 task_id（`dylw<ms>` 前缀避免与 download 的 `dy<ms>` 冲突），
/// 落 job + 初始 queued status，spawn 脱离的 worker 子进程，立即返回。
///
/// 调用方应在 status 返回 queued 后等 5s 再首次 status 轮询（worker 启动 + 第一页 API
/// 通常 1-3s）。
pub fn submit(
    task_dir: &Path,
    cookie_file: &Path,
    handle: String,
    max_pages: usize,
    delivery_handle: Option<String>,
    session_id: Option<String>,
) -> Result<TaskStatus> {
    std::fs::create_dir_all(task_dir)
        .with_context(|| format!("建任务目录 {}", task_dir.display()))?;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let task_id = format!("dylw{ms}");

    let job = Job {
        task_id: task_id.clone(),
        handle: handle.clone(),
        max_pages,
        cookie_file: cookie_file.to_path_buf(),
        delivery_handle,
        session_id,
    };
    atomic_write(&job_path(task_dir, &task_id), &serde_json::to_string(&job)?)?;

    let st = TaskStatus {
        task_id: task_id.clone(),
        state: "queued".into(),
        sec_uid: None,
        nickname: None,
        unique_id: None,
        pages_fetched: 0,
        max_pages,
        aweme_count: -1,
        count: 0,
        throttled: false,
        works: vec![],
        error: None,
        updated_at: now(),
        heartbeat_at: None,
        notified: false,
    };
    write_status(task_dir, &st)?;

    spawn_worker(task_dir, &task_id)?;

    Ok(st)
}

/// spawn 脱离的 list-works worker 子进程（隐藏子命令）。父进程退出后子进程继续。
/// submit / retry / reap 共用。test 下不真正起进程。
fn spawn_worker(task_dir: &Path, task_id: &str) -> Result<()> {
    #[cfg(not(test))]
    {
        let exe = std::env::current_exe().context("取当前可执行路径")?;
        std::process::Command::new(exe)
            .arg("list-works-worker")
            .arg("--task-dir")
            .arg(task_dir)
            .arg("--task-id")
            .arg(task_id)
            .spawn()
            .context("spawn list-works worker")?;
    }
    #[cfg(test)]
    {
        let _ = (task_dir, task_id);
    }
    Ok(())
}

/// 重启一个列作品任务：标回 queued 并重 spawn worker。worker 重头翻页重建结果
/// （list 无逐项缓存，retry 即整任务重跑）。返回 None 表示 job 不存在。
pub fn retry(task_dir: &Path, task_id: &str) -> Result<Option<TaskStatus>> {
    if !job_path(task_dir, task_id).exists() {
        return Ok(None);
    }
    clear_cancel(task_dir, task_id); // 重跑前清掉残留 cancel 标志
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

fn cancel_flag_path(task_dir: &Path, task_id: &str) -> PathBuf {
    task_dir.join(format!("{task_id}.cancel"))
}

/// 请求取消任务：写 cancel 标志，worker 翻下一页前检查并转 cancelled。
/// 仅 queued/running 有意义；返回 false 表示不存在或已终态。
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

/// 扫描并重启心跳超时（stale）的 running 列作品任务。返回被 reap 的 task_id。
pub fn reap(task_dir: &Path, stale_secs: i64) -> Result<Vec<String>> {
    let ids = stale_running_task_ids(task_dir, stale_secs)?;
    for id in &ids {
        retry(task_dir, id)?;
    }
    Ok(ids)
}

/// 列出 stale 的 running list-works 任务 id（前缀 `dylw`）。
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
        let Some(task_id) = name.strip_suffix(".status.json") else {
            continue;
        };
        if !task_id.starts_with("dylw") {
            continue;
        }
        let Ok(Some(st)) = read_status(task_dir, task_id) else {
            continue;
        };
        if is_stale_running(&st, stale_secs, now) {
            out.push(task_id.to_string());
        }
    }
    Ok(out)
}

/// 判定 stale running：state==running 且心跳（缺则退化用 updated_at）距今 ≥ stale_secs。
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

/// worker 入口：读 job，resolve sec_uid，循环翻页直到 has_more 结束或撞 max_pages；
/// 每页 write_status 让 status 调用看进度。
///
/// 翻页循环复制自 api.rs `list_all_works`（30 行）——抽 progress callback 会污染
/// api 接口，按 RFC §2 风险项决策直接重复。终态把 works 一次性写入 status。
pub async fn run_worker(task_dir: &Path, task_id: &str) -> Result<()> {
    let job: Job = {
        let raw = std::fs::read_to_string(job_path(task_dir, task_id)).context("读 job 文件")?;
        serde_json::from_str(&raw).context("解析 job")?
    };

    // ===== queued → running 之前的预处理：cookie + sec_uid =====
    let cookies = match crate::load_cookie_file(&job.cookie_file) {
        Ok(c) => c,
        Err(e) => {
            write_failed(
                task_dir,
                task_id,
                job.max_pages,
                format!("读 cookie 文件: {e}"),
            )?;
            return Ok(());
        }
    };
    let client = match DouyinClient::from_cookies(&cookies) {
        Ok(c) => c,
        Err(e) => {
            write_failed(
                task_dir,
                task_id,
                job.max_pages,
                format!("cookie 不可用: {e}"),
            )?;
            return Ok(());
        }
    };
    let sec_uid = match crate::resolve_to_sec_uid(&client, &job.handle).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            write_failed(
                task_dir,
                task_id,
                job.max_pages,
                "无法解析 sec_uid（invalid_input）".into(),
            )?;
            return Ok(());
        }
        Err(e) => {
            write_failed(
                task_dir,
                task_id,
                job.max_pages,
                format!("resolve sec_uid: {}", e.message),
            )?;
            return Ok(());
        }
    };
    // 同时拿 nickname + aweme_count + unique_id，写入 TaskStatus 让回调路径不必再 resolve_user，
    // 并作为知识包目录键（Plan 5）。
    let (nickname, aweme_count, unique_id) = client
        .user_profile(&sec_uid)
        .await
        .map(|(name, c, user)| {
            let uid = user
                .get("unique_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            (Some(name), c, uid)
        })
        .unwrap_or((None, -1, None));

    // ===== 切到 running，开始翻页 =====
    let mut st = TaskStatus {
        task_id: task_id.into(),
        state: "running".into(),
        sec_uid: Some(sec_uid.clone()),
        nickname,
        unique_id,
        pages_fetched: 0,
        max_pages: job.max_pages,
        aweme_count,
        count: 0,
        throttled: false,
        works: vec![],
        error: None,
        updated_at: now(),
        heartbeat_at: Some(now()),
        notified: false,
    };
    write_status(task_dir, &st)?;

    // ===== 翻页循环（复制 api.rs::list_all_works，每页 write_status）=====
    let mut cursor = 0i64;
    let mut all: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut shadow_throttle = false;
    loop {
        if st.pages_fetched >= job.max_pages {
            break;
        }
        // 取消检查：翻下一页前看 cancel 标志，命中则转 cancelled 干净退出。
        if is_cancelled(task_dir, task_id) {
            clear_cancel(task_dir, task_id);
            st.state = "cancelled".into();
            st.updated_at = now();
            let _ = write_status(task_dir, &st);
            log::info!("[list-works] cancelled task_id={task_id}");
            return Ok(());
        }
        let page = match client.user_post_page(&sec_uid, cursor).await {
            Ok(p) => p,
            Err(e) => {
                // 第一页就失败 → failed；中途失败 → partial（保留已拉的）。
                if st.pages_fetched == 0 {
                    write_failed(
                        task_dir,
                        task_id,
                        job.max_pages,
                        format!("首页 API: {}", e.message),
                    )?;
                    return Ok(());
                } else {
                    st.error = Some(format!(
                        "中途 API 失败（第 {} 页）: {}",
                        st.pages_fetched + 1,
                        e.message
                    ));
                    break;
                }
            }
        };
        st.pages_fetched += 1;
        for a in &page.items {
            if let Some(id) = a.get("aweme_id").and_then(|v| v.as_str()) {
                if seen.insert(id.to_string()) {
                    all.push(a.clone());
                }
            }
        }
        // shadow-throttle 信号：页面给了游标骨架但 items 被抽稀。
        if !page.items.is_empty() && page.items.len() < 5 {
            shadow_throttle = true;
        }
        st.count = all.len();
        st.throttled = shadow_throttle;
        st.updated_at = now();
        st.heartbeat_at = Some(now());
        let _ = write_status(task_dir, &st);
        if !page.has_more || page.max_cursor == cursor {
            break;
        }
        cursor = page.max_cursor;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // ===== 终态：与 run_list_works 同款 throttled 判定（确定性抽稀） =====
    let determinate_throttled = aweme_count > 0 && (all.len() as i64) < aweme_count * 9 / 10;
    let final_throttled = shadow_throttle || determinate_throttled;

    let items: Vec<Value> = all
        .iter()
        .map(|a| {
            let ts = a.get("create_time").and_then(|v| v.as_i64()).unwrap_or(0);
            let ym = chrono::DateTime::from_timestamp(ts, 0)
                .map(|d| d.format("%Y-%m").to_string())
                .unwrap_or_default();
            let mut item = serde_json::json!({
                "aweme_id": a.get("aweme_id"),
                "desc": a.get("desc"),
                "create_time": a.get("create_time"),
                "create_ym": ym,
            });
            crate::knowledge::enrich_with_tags(&mut item);
            item
        })
        .collect();

    st.count = items.len();
    st.throttled = final_throttled;
    st.works = items;
    st.state = if st.error.is_some() {
        // 中途失败但有部分数据 → partial
        "partial".into()
    } else if final_throttled && aweme_count > 0 && (st.count as i64) < aweme_count {
        "partial".into()
    } else {
        "succeeded".into()
    };
    st.updated_at = now();
    write_status(task_dir, &st)?;

    // ===== Plan 5：终态落「按 unique_id 的稳定缓存」，供 list_tags / filter_works /
    // publish_knowledge 使用。缓存目录与 tasks 同级（task_dir 的兄弟 works/）。
    // unique_id 缺失时退化用 sec_uid 命名，保证调用方两种 id 都能命中。
    if st.state == "succeeded" || st.state == "partial" {
        let works_dir = task_dir
            .parent()
            .map(|p| p.join("works"))
            .unwrap_or_else(|| task_dir.join("works"));
        let cache_id = st.unique_id.clone().unwrap_or_else(|| sec_uid.clone());
        let cache = crate::knowledge::WorksCache {
            sec_uid: sec_uid.clone(),
            unique_id: st.unique_id.clone(),
            nickname: st.nickname.clone(),
            aweme_count: st.aweme_count,
            count: st.count,
            throttled: st.throttled,
            cached_at: now(),
            works: st.works.clone(),
        };
        if let Err(e) = crate::knowledge::write_cache(&works_dir, &cache_id, &cache) {
            log::warn!("[list-works] 写作品缓存失败 id={cache_id}: {e}");
        } else {
            log::info!(
                "[list-works] 作品缓存已落盘 id={cache_id} count={}",
                st.count
            );
        }
    }

    // ===== 业务回调：入持久队列并当场尝试投递，让第二轮 LLM 周期接管 =====
    // 详见 docs/2026-05-31-callback-driven-async-tasks/。未当场送达则留 pending，
    // 由 callback-flush 补发（修掉 §4.4 通知永久丢失）。
    // delivery_handle 缺失时（CLI 手测场景）跳过回调，只落 status。
    if let Some(handle) = &job.delivery_handle {
        let kind = if st.state == "failed" {
            "douyin-list-works-failed"
        } else {
            "douyin-list-works-done"
        };
        let mut payload = serde_json::Map::new();
        payload.insert("task_id".into(), serde_json::json!(st.task_id));
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
                // 持久化 notified=true 让 alarm 兜底子 Agent 据此走"静默退出"。
                st.notified = true;
                st.updated_at = now();
                let _ = write_status(task_dir, &st);
                log::info!(
                    "[list-works callback] notified=true persisted task_id={}",
                    st.task_id
                );
            }
            Ok(false) => {
                log::warn!(
                    "[list-works callback] 未当场送达，已入队待 flush task_id={}",
                    st.task_id
                );
            }
            Err(e) => {
                log::warn!(
                    "[list-works callback] enqueue/deliver failed task_id={}: {e}",
                    st.task_id
                );
            }
        }
    }
    Ok(())
}

/// 写一条 failed 终态，不返回错误（保证 worker 进程干净退出）。
fn write_failed(task_dir: &Path, task_id: &str, max_pages: usize, error: String) -> Result<()> {
    let st = TaskStatus {
        task_id: task_id.into(),
        state: "failed".into(),
        sec_uid: None,
        nickname: None,
        unique_id: None,
        pages_fetched: 0,
        max_pages,
        aweme_count: -1,
        count: 0,
        throttled: false,
        works: vec![],
        error: Some(error),
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
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "douyin-list-works-test-{}-{}",
            std::process::id(),
            id
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn cleanup(p: &Path) {
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn read_status_returns_none_for_unknown_task() {
        let dir = tempdir();
        let result = read_status(&dir, "dylw404404").unwrap();
        assert!(result.is_none());
        cleanup(&dir);
    }

    #[test]
    fn write_then_read_status_roundtrip() {
        let dir = tempdir();
        let st = TaskStatus {
            task_id: "dylwtest".into(),
            state: "running".into(),
            sec_uid: Some("MS4wTEST".into()),
            nickname: Some("熊猫怪兽AI日记".into()),
            unique_id: Some("82933463317".into()),
            pages_fetched: 3,
            max_pages: 60,
            aweme_count: 81,
            count: 54,
            throttled: false,
            works: vec![],
            error: None,
            updated_at: "2026-05-31T00:00:00Z".into(),
            heartbeat_at: None,
            notified: false,
        };
        write_status(&dir, &st).unwrap();
        let read = read_status(&dir, "dylwtest").unwrap().unwrap();
        assert_eq!(read.task_id, "dylwtest");
        assert_eq!(read.state, "running");
        assert_eq!(read.pages_fetched, 3);
        assert_eq!(read.sec_uid.as_deref(), Some("MS4wTEST"));
        assert_eq!(read.nickname.as_deref(), Some("熊猫怪兽AI日记"));
        assert_eq!(read.aweme_count, 81);
        assert_eq!(read.count, 54);
        assert!(!read.notified);
        cleanup(&dir);
    }

    /// Plan 2 新增：Job 携带 delivery_handle 时 submit 应原样写入 job 文件。
    #[test]
    fn submit_persists_delivery_handle_in_job() {
        let dir = tempdir();
        let cookie = dir.join("fake-cookie.json");
        std::fs::write(&cookie, "{}").unwrap();
        // submit 会 spawn worker——但 worker 父进程退出后子进程靠 stdin/job 文件跑，
        // 测试只关心 job 文件落盘内容（worker 跑不跑通是集成测试范畴）。
        let st = submit(
            &dir,
            &cookie,
            "https://example.com/user/x".into(),
            60,
            Some("dh_test_handle".into()),
            None,
        )
        .unwrap();
        let job_str = std::fs::read_to_string(job_path(&dir, &st.task_id)).unwrap();
        let job: Job = serde_json::from_str(&job_str).unwrap();
        assert_eq!(job.delivery_handle.as_deref(), Some("dh_test_handle"));
        cleanup(&dir);
    }

    /// Plan 3 新增：TaskStatus 序列化/反序列化往返保留 notified=true 与 nickname。
    #[test]
    fn task_status_serde_with_notified_and_nickname() {
        let dir = tempdir();
        let st = TaskStatus {
            task_id: "dylwfull".into(),
            state: "succeeded".into(),
            sec_uid: Some("MS4w".into()),
            nickname: Some("Nick".into()),
            unique_id: Some("nick123".into()),
            pages_fetched: 5,
            max_pages: 60,
            aweme_count: 81,
            count: 81,
            throttled: false,
            works: vec![serde_json::json!({"aweme_id": "1"})],
            error: None,
            updated_at: "2026-05-31T00:00:00Z".into(),
            heartbeat_at: None,
            notified: true,
        };
        write_status(&dir, &st).unwrap();
        let read = read_status(&dir, "dylwfull").unwrap().unwrap();
        assert!(read.notified);
        assert_eq!(read.nickname.as_deref(), Some("Nick"));
        assert_eq!(read.works.len(), 1);
        cleanup(&dir);
    }

    /// Plan 2 新增：session_id 字段原样落盘 job 文件。
    #[test]
    fn submit_persists_session_id_in_job() {
        let dir = tempdir();
        let cookie = dir.join("fake-cookie.json");
        std::fs::write(&cookie, "{}").unwrap();
        let st = submit(
            &dir,
            &cookie,
            "https://example.com/user/x".into(),
            60,
            Some("dh_test".into()),
            Some("test-uuid-abcd".into()),
        )
        .unwrap();
        let job_str = std::fs::read_to_string(job_path(&dir, &st.task_id)).unwrap();
        let job: Job = serde_json::from_str(&job_str).unwrap();
        assert_eq!(job.session_id.as_deref(), Some("test-uuid-abcd"));
        cleanup(&dir);
    }

    fn running_status(task_id: &str, heartbeat: &str) -> TaskStatus {
        TaskStatus {
            task_id: task_id.into(),
            state: "running".into(),
            sec_uid: Some("MS4w".into()),
            nickname: None,
            unique_id: None,
            pages_fetched: 1,
            max_pages: 60,
            aweme_count: -1,
            count: 0,
            throttled: false,
            works: vec![],
            error: None,
            updated_at: heartbeat.into(),
            heartbeat_at: Some(heartbeat.into()),
            notified: false,
        }
    }

    #[test]
    fn is_stale_running_detects_timeout() {
        let now = chrono::Utc::now();
        let old = (now - chrono::Duration::seconds(1000)).to_rfc3339();
        let fresh = (now - chrono::Duration::seconds(10)).to_rfc3339();
        assert!(is_stale_running(&running_status("dylw1", &old), 600, now));
        assert!(!is_stale_running(&running_status("dylw1", &fresh), 600, now));
        let mut done = running_status("dylw1", &old);
        done.state = "succeeded".into();
        assert!(!is_stale_running(&done, 600, now));
    }

    #[test]
    fn stale_scan_picks_only_listworks_running() {
        let dir = tempdir();
        let now = chrono::Utc::now();
        let old = (now - chrono::Duration::seconds(1000)).to_rfc3339();
        let fresh = now.to_rfc3339();
        write_status(&dir, &running_status("dylw_stale", &old)).unwrap();
        write_status(&dir, &running_status("dylw_fresh", &fresh)).unwrap();
        let mut done = running_status("dylw_done", &old);
        done.state = "partial".into();
        write_status(&dir, &done).unwrap();
        let ids = stale_running_task_ids(&dir, 600).unwrap();
        assert_eq!(ids, vec!["dylw_stale".to_string()]);
        cleanup(&dir);
    }

    #[test]
    fn retry_missing_job_returns_none() {
        let dir = tempdir();
        assert!(retry(&dir, "dylw404").unwrap().is_none());
        cleanup(&dir);
    }

    #[test]
    fn cancel_writes_flag_only_for_active_tasks() {
        let dir = tempdir();
        assert!(!cancel(&dir, "dylw_x").unwrap());
        write_status(&dir, &running_status("dylw1", &now())).unwrap();
        assert!(cancel(&dir, "dylw1").unwrap());
        assert!(is_cancelled(&dir, "dylw1"));
        clear_cancel(&dir, "dylw1");
        assert!(!is_cancelled(&dir, "dylw1"));
        cleanup(&dir);
    }

    #[test]
    fn old_status_without_heartbeat_deserializes() {
        let raw = r#"{"task_id":"dylw1","state":"running","sec_uid":null,
            "pages_fetched":0,"max_pages":60,"aweme_count":-1,"count":0,
            "throttled":false,"works":[],"error":null,"updated_at":"2026-05-31T00:00:00Z"}"#;
        let st: TaskStatus = serde_json::from_str(raw).unwrap();
        assert!(st.heartbeat_at.is_none());
    }

    #[test]
    fn atomic_write_replaces_existing_content() {
        let dir = tempdir();
        let path = dir.join("x.json");
        atomic_write(&path, "{\"v\":1}").unwrap();
        atomic_write(&path, "{\"v\":2}").unwrap();
        let s = std::fs::read_to_string(&path).unwrap();
        assert_eq!(s, "{\"v\":2}");
        cleanup(&dir);
    }

    /// task_id 前缀避免与 download.rs 的 `dy<ms>` 冲突——构造一个 download 风格 task_id
    /// 用 list-works read_status 查应该 None（前缀不匹配）。
    #[test]
    fn distinct_task_id_namespace_from_download() {
        let dir = tempdir();
        // 模拟 download 写了个 dy* 状态文件
        let st = TaskStatus {
            task_id: "dy1780000000".into(),
            state: "succeeded".into(),
            sec_uid: None,
            nickname: None,
            unique_id: None,
            pages_fetched: 0,
            max_pages: 0,
            aweme_count: -1,
            count: 0,
            throttled: false,
            works: vec![],
            error: None,
            updated_at: "2026-05-31T00:00:00Z".into(),
            heartbeat_at: None,
            notified: false,
        };
        write_status(&dir, &st).unwrap();
        // list-works 查 dylw* 应该 None（即便 dir 里有 dy* 文件）
        assert!(read_status(&dir, "dylw1780000000").unwrap().is_none());
        // 但用真 task_id 能读出来——证明文件结构相同，只靠 task_id 前缀区分
        assert!(read_status(&dir, "dy1780000000").unwrap().is_some());
        cleanup(&dir);
    }
}
