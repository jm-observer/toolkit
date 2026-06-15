//! 等待会话空闲：轮询会话文件直到状态 Idle 且文件 mtime 稳定。
//!
//! 用途（见 plan.md §5）：`wait_for_claude_idle=true` 时先等当前（用户驱动的）轮次结束再
//! 接管；循环内 `send` 走阻塞 CLI，本模块仅作完成性兜底校验。
//!
//! 判定：复用 [`Store::snapshot`] 得到 [`SessionStatus`]——状态为 `Idle` 且连续两次轮询
//! 文件 mtime 不变，视为本轮结束（叠加 mtime 兜底，规避「状态已 Idle 但文件仍在刷新」）。

use crate::store::Store;
use crate::{Provider, SessionRef, SessionStatus};
use anyhow::{anyhow, Result};
use std::path::Path;
use std::time::{Duration, SystemTime};

/// 轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// 取会话文件 mtime（缺失 / 读不到回退 UNIX_EPOCH，与 store 内部一致）。
fn file_mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// 判断一次轮询是否「已空闲且稳定」：状态 Idle 且 mtime 与上次相同。
///
/// 纯函数，便于单测（不依赖真进程 / 真 sleep）。
pub fn is_idle_stable(
    status: SessionStatus,
    prev_mtime: SystemTime,
    cur_mtime: SystemTime,
) -> bool {
    status == SessionStatus::Idle && prev_mtime == cur_mtime
}

/// 轮询指定会话，直到状态 Idle 且 mtime 稳定，或超时。
///
/// 超时返回 `Err`（调用方据语义决定是否当业务终态 AbortedTimeout 处理）。
pub async fn wait_for_idle(store: &Store, s: &SessionRef, timeout: Duration) -> Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    let mut prev_mtime: Option<SystemTime> = None;
    loop {
        let path = store
            .locate(s.provider, &s.session_id)?
            .ok_or_else(|| anyhow!("未找到会话文件: {}", s.session_id))?;
        let status = snapshot_status(store, s.provider, &s.session_id)?;
        let cur_mtime = file_mtime(&path);
        if let Some(prev) = prev_mtime {
            if is_idle_stable(status, prev, cur_mtime) {
                return Ok(());
            }
        }
        prev_mtime = Some(cur_mtime);
        if std::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "等待会话 {} 空闲超时（{:?}）",
                s.session_id,
                timeout
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn snapshot_status(store: &Store, provider: Provider, id: &str) -> Result<SessionStatus> {
    Ok(store.snapshot(provider, id)?.status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_stable_requires_idle_and_same_mtime() {
        let t0 = SystemTime::UNIX_EPOCH;
        let t1 = t0 + Duration::from_secs(1);
        // Idle + 同 mtime → 稳定
        assert!(is_idle_stable(SessionStatus::Idle, t0, t0));
        // Idle 但 mtime 变化 → 仍在刷新
        assert!(!is_idle_stable(SessionStatus::Idle, t0, t1));
        // 非 Idle → 未结束
        assert!(!is_idle_stable(SessionStatus::Generating, t0, t0));
        assert!(!is_idle_stable(SessionStatus::Processing, t0, t0));
    }

    #[tokio::test]
    async fn wait_returns_ok_for_idle_fixture() {
        // 用 Plan 1 fixture 的已完成 codex 会话：状态 Idle，两次轮询间文件不变 → 应立即（第二轮）返回。
        let home = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let store = Store::with_home(home);
        let s = SessionRef {
            provider: Provider::Codex,
            session_id: "11111111-aaaa-bbbb-cccc-000000000001".to_string(),
            cwd: std::path::PathBuf::new(),
        };
        let r = wait_for_idle(&store, &s, Duration::from_secs(10)).await;
        assert!(r.is_ok(), "已完成的 idle 会话应返回 Ok: {r:?}");
    }

    #[tokio::test]
    async fn wait_times_out_for_generating_fixture() {
        // running 会话状态 Generating，永不 idle → 应超时 Err（超时设短）。
        let home = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let store = Store::with_home(home);
        let s = SessionRef {
            provider: Provider::Codex,
            session_id: "22222222-aaaa-bbbb-cccc-000000000002".to_string(),
            cwd: std::path::PathBuf::new(),
        };
        let r = wait_for_idle(&store, &s, Duration::from_millis(1)).await;
        assert!(r.is_err(), "Generating 会话应超时");
    }
}
