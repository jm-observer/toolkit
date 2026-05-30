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
    pub pages_fetched: usize,
    pub max_pages: usize,
    pub aweme_count: i64,
    pub count: usize,
    pub throttled: bool,
    pub works: Vec<Value>,
    pub error: Option<String>,
    pub updated_at: String,
}

/// 任务作业描述（submit 写、worker 读）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Job {
    pub task_id: String,
    pub handle: String,
    pub max_pages: usize,
    pub cookie_file: PathBuf,
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
    };
    atomic_write(&job_path(task_dir, &task_id), &serde_json::to_string(&job)?)?;

    let st = TaskStatus {
        task_id: task_id.clone(),
        state: "queued".into(),
        sec_uid: None,
        pages_fetched: 0,
        max_pages,
        aweme_count: -1,
        count: 0,
        throttled: false,
        works: vec![],
        error: None,
        updated_at: now(),
    };
    write_status(task_dir, &st)?;

    // spawn 脱离的 worker：同款 list-works-worker 隐藏子命令。父进程退出后子进程继续。
    let exe = std::env::current_exe().context("取当前可执行路径")?;
    std::process::Command::new(exe)
        .arg("list-works-worker")
        .arg("--task-dir")
        .arg(task_dir)
        .arg("--task-id")
        .arg(&task_id)
        .spawn()
        .context("spawn list-works worker")?;

    Ok(st)
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
    let aweme_count = client
        .user_profile(&sec_uid)
        .await
        .map(|(_, c, _)| c)
        .unwrap_or(-1);

    // ===== 切到 running，开始翻页 =====
    let mut st = TaskStatus {
        task_id: task_id.into(),
        state: "running".into(),
        sec_uid: Some(sec_uid.clone()),
        pages_fetched: 0,
        max_pages: job.max_pages,
        aweme_count,
        count: 0,
        throttled: false,
        works: vec![],
        error: None,
        updated_at: now(),
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
            serde_json::json!({
                "aweme_id": a.get("aweme_id"),
                "desc": a.get("desc"),
                "create_time": a.get("create_time"),
                "create_ym": ym,
            })
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
    Ok(())
}

/// 写一条 failed 终态，不返回错误（保证 worker 进程干净退出）。
fn write_failed(task_dir: &Path, task_id: &str, max_pages: usize, error: String) -> Result<()> {
    let st = TaskStatus {
        task_id: task_id.into(),
        state: "failed".into(),
        sec_uid: None,
        pages_fetched: 0,
        max_pages,
        aweme_count: -1,
        count: 0,
        throttled: false,
        works: vec![],
        error: Some(error),
        updated_at: now(),
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
            pages_fetched: 3,
            max_pages: 60,
            aweme_count: 81,
            count: 54,
            throttled: false,
            works: vec![],
            error: None,
            updated_at: "2026-05-31T00:00:00Z".into(),
        };
        write_status(&dir, &st).unwrap();
        let read = read_status(&dir, "dylwtest").unwrap().unwrap();
        assert_eq!(read.task_id, "dylwtest");
        assert_eq!(read.state, "running");
        assert_eq!(read.pages_fetched, 3);
        assert_eq!(read.sec_uid.as_deref(), Some("MS4wTEST"));
        assert_eq!(read.aweme_count, 81);
        assert_eq!(read.count, 54);
        cleanup(&dir);
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
            pages_fetched: 0,
            max_pages: 0,
            aweme_count: -1,
            count: 0,
            throttled: false,
            works: vec![],
            error: None,
            updated_at: "2026-05-31T00:00:00Z".into(),
        };
        write_status(&dir, &st).unwrap();
        // list-works 查 dylw* 应该 None（即便 dir 里有 dy* 文件）
        assert!(read_status(&dir, "dylw1780000000").unwrap().is_none());
        // 但用真 task_id 能读出来——证明文件结构相同，只靠 task_id 前缀区分
        assert!(read_status(&dir, "dy1780000000").unwrap().is_some());
        cleanup(&dir);
    }
}
