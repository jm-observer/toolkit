//! 异步下载模型（v1 保留）：`submit` 入队立返 task_id；后台 worker 进程逐个下载并
//! **原子替换**状态文件；`status` 读状态。绕开 nova 30s 硬超时。
//!
//! 状态机：queued → running → (succeeded | partial | failed)。

use crate::api::DouyinClient;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 下载任务状态（落盘 JSON）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaskStatus {
    pub task_id: String,
    pub state: String,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub files: Vec<String>,
    pub errors: Vec<String>,
    pub updated_at: String,
}

/// 任务作业描述（submit 写、worker 读）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Job {
    pub task_id: String,
    pub ids: Vec<String>,
    pub cookie_file: PathBuf,
    pub out_dir: PathBuf,
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

/// 原子写：先写 .tmp 再 rename，避免 status 读到半截。
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

/// 入队：生成 task_id，落 job + 初始 status，spawn 脱离的 worker 进程，立即返回。
pub fn submit(
    task_dir: &Path,
    out_dir: &Path,
    cookie_file: &Path,
    ids: Vec<String>,
) -> Result<TaskStatus> {
    std::fs::create_dir_all(task_dir)
        .with_context(|| format!("建任务目录 {}", task_dir.display()))?;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let task_id = format!("dy{ms}");

    let job = Job {
        task_id: task_id.clone(),
        ids: ids.clone(),
        cookie_file: cookie_file.to_path_buf(),
        out_dir: out_dir.to_path_buf(),
    };
    atomic_write(&job_path(task_dir, &task_id), &serde_json::to_string(&job)?)?;

    let st = TaskStatus {
        task_id: task_id.clone(),
        state: "queued".into(),
        total: ids.len(),
        done: 0,
        failed: 0,
        files: vec![],
        errors: vec![],
        updated_at: now(),
    };
    write_status(task_dir, &st)?;

    // spawn 脱离的 worker：同一二进制的隐藏子命令。父进程退出后子进程继续。
    #[cfg(not(test))]
    {
        let exe = std::env::current_exe().context("取当前可执行路径")?;
        std::process::Command::new(exe)
            .arg("download-worker")
            .arg("--task-dir")
            .arg(task_dir)
            .arg("--task-id")
            .arg(&task_id)
            .spawn()
            .context("spawn 下载 worker")?;
    }

    Ok(st)
}

/// worker 入口：读 job，逐个下载，原子更新 status。由 submit spawn，独立进程运行。
pub async fn run_worker(task_dir: &Path, task_id: &str) -> Result<()> {
    let job: Job = {
        let raw = std::fs::read_to_string(job_path(task_dir, task_id)).context("读 job 文件")?;
        serde_json::from_str(&raw).context("解析 job")?
    };
    std::fs::create_dir_all(&job.out_dir).ok();

    let cookies = crate::load_cookie_file(&job.cookie_file)?;
    let client = match DouyinClient::from_cookies(&cookies) {
        Ok(c) => c,
        Err(e) => {
            let st = TaskStatus {
                task_id: task_id.into(),
                state: "failed".into(),
                total: job.ids.len(),
                done: 0,
                failed: job.ids.len(),
                files: vec![],
                errors: vec![format!("cookie 不可用: {e}")],
                updated_at: now(),
            };
            write_status(task_dir, &st)?;
            return Ok(());
        }
    };

    let mut st = TaskStatus {
        task_id: task_id.into(),
        state: "running".into(),
        total: job.ids.len(),
        done: 0,
        failed: 0,
        files: vec![],
        errors: vec![],
        updated_at: now(),
    };
    write_status(task_dir, &st)?;

    let http = reqwest::Client::new();
    for id in &job.ids {
        match download_one(&client, &http, id, &job.out_dir).await {
            Ok(path) => {
                st.done += 1;
                st.files.push(path);
            }
            Err(e) => {
                st.failed += 1;
                st.errors.push(format!("{id}: {e}"));
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
    Ok(())
}

/// 下载单个作品的无水印 mp4 到 `out_dir/<aweme_id>.mp4`，返回落盘绝对路径字符串。
///
/// 幂等：最终 `.mp4` 存在即视为已完成并跳过。这一判断之所以可信，是因为落盘走
/// `.partial` + atomic rename（见 [`finalize_download`]）——崩溃只会留下 `.partial`，
/// 最终 `.mp4` 永远不会是半截文件。供 process worker 复用。
pub(crate) async fn download_one(
    client: &DouyinClient,
    http: &reqwest::Client,
    aweme_id: &str,
    out_dir: &Path,
) -> Result<String> {
    let existing = out_dir.join(format!("{aweme_id}.mp4"));
    if existing.exists() {
        return Ok(existing.to_string_lossy().to_string());
    }
    let (_, urls, _) = client
        .aweme_detail(aweme_id)
        .await
        .map_err(|e| anyhow::anyhow!("详情失败: {e}"))?;
    let url = urls.first().context("无 play_addr 下载 URL")?;
    let resp = http
        .get(url)
        .header("user-agent", crate::api::UA)
        .header("referer", "https://www.douyin.com/")
        .send()
        .await
        .context("下载请求")?;
    let declared_len = resp.content_length();
    let bytes = resp.bytes().await.context("读下载内容")?;
    finalize_download(out_dir, aweme_id, &bytes, declared_len)
}

/// 校验 + 原子落盘：核对 Content-Length（若服务端声明）→ 写 `<id>.mp4.partial` →
/// atomic rename 到 `<id>.mp4`。
///
/// 三道闸口防止"半截/损坏/空文件被当成已完成"：
/// 1. 空内容直接判失败，不留空 mp4（否则会被 `exists()` 永久误判为已下载）。
/// 2. 声明了 Content-Length 时核对字节数，截断响应不落盘。
/// 3. 先写 `.partial` 再 rename，最终 `.mp4` 只在完整写入后出现。
fn finalize_download(
    out_dir: &Path,
    aweme_id: &str,
    bytes: &[u8],
    declared_len: Option<u64>,
) -> Result<String> {
    if bytes.is_empty() {
        anyhow::bail!("下载内容为空");
    }
    if let Some(len) = declared_len {
        if bytes.len() as u64 != len {
            anyhow::bail!("下载不完整：声明 {len} 字节，实收 {} 字节", bytes.len());
        }
    }
    let out = out_dir.join(format!("{aweme_id}.mp4"));
    let partial = out_dir.join(format!("{aweme_id}.mp4.partial"));
    std::fs::write(&partial, bytes).with_context(|| format!("写 {}", partial.display()))?;
    std::fs::rename(&partial, &out).with_context(|| format!("替换 {}", out.display()))?;
    Ok(out.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        let id = C.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("douyin-dl-test-{}-{}", std::process::id(), id));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn finalize_writes_final_and_leaves_no_partial() {
        let dir = tempdir();
        let bytes = b"fake mp4 bytes";
        let path = finalize_download(&dir, "7a", bytes, Some(bytes.len() as u64)).unwrap();
        assert!(dir.join("7a.mp4").exists());
        assert!(!dir.join("7a.mp4.partial").exists());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finalize_rejects_empty() {
        let dir = tempdir();
        assert!(finalize_download(&dir, "7a", b"", None).is_err());
        // 不留空 mp4，否则会被 exists() 永久误判为已下载。
        assert!(!dir.join("7a.mp4").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finalize_rejects_length_mismatch() {
        let dir = tempdir();
        // 声明 100 字节但只收到 5 字节（截断响应）。
        let err = finalize_download(&dir, "7a", b"short", Some(100));
        assert!(err.is_err());
        assert!(!dir.join("7a.mp4").exists());
        assert!(!dir.join("7a.mp4.partial").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finalize_overwrites_stale_partial() {
        let dir = tempdir();
        // 上次崩溃残留的 .partial，应被新一次写入覆盖、不致 rename 失败。
        std::fs::write(dir.join("7a.mp4.partial"), b"stale crash leftover").unwrap();
        let bytes = b"fresh complete bytes";
        finalize_download(&dir, "7a", bytes, None).unwrap();
        assert_eq!(std::fs::read(dir.join("7a.mp4")).unwrap(), bytes);
        assert!(!dir.join("7a.mp4.partial").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
