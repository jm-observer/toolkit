//! zero 的 HuggingFace 趋势监听工具库。
//!
//! 两个子命令对应两个 zero 工具：
//! - `trending`：取某 pipeline_tag 的 trending top-N，与上次快照对比，输出新进榜 / 出新版本的模型。
//! - `model-card`：取单模型 README 原文 + meta（参数量等），供子 Agent 判别与对比。
//!
//! 输出契约：紧凑 JSON 到 stdout；业务失败（网络 / 404 / 解析）输出
//! `{error, error_kind}` 且 **退出码 0**；仅进程级异常退出码非 0。

pub mod api;
pub mod snapshot;

use anyhow::Result;
use api::{ApiError, HfClient};
use serde_json::{json, Value};
use std::path::Path;

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn api_error_json(e: &ApiError) -> Value {
    json!({ "error": e.message, "error_kind": e.kind })
}

/// `trending` 子命令：取榜 + 对比快照 + （默认）回写快照。
///
/// `snapshot_dir` 由调用方（zero agent）传入，工具本身**不做任何默认值 / 环境变量 /
/// cwd 回退**——目录归属由 agent 决定。
pub async fn run_trending(
    pipeline_tag: &str,
    top_n: usize,
    snapshot_dir: &Path,
    write: bool,
) -> Result<Value> {
    let client = HfClient::new()?;
    let entries = match client.trending(pipeline_tag, top_n).await {
        Ok(e) => e,
        Err(e) => {
            log::warn!(
                "trending[{pipeline_tag}] failed: {} ({})",
                e.message,
                e.kind
            );
            return Ok(api_error_json(&e));
        }
    };

    let path = snapshot::snapshot_path(snapshot_dir, pipeline_tag);
    let prev = snapshot::load(&path)?;
    let diff = snapshot::diff(prev.as_ref(), &entries);
    log::info!(
        "trending[{pipeline_tag}] top_n={top_n} fetched={} newcomers={} updated={} unchanged={} first_run={}",
        entries.len(),
        diff.newcomers.len(),
        diff.updated.len(),
        diff.unchanged_count,
        diff.first_run,
    );

    let new_snap = snapshot::build_snapshot(pipeline_tag, top_n, now_rfc3339(), &entries);
    let mut written = false;
    if write {
        snapshot::save(&path, &new_snap)?;
        written = true;
        log::debug!(
            "trending[{pipeline_tag}] snapshot written to {}",
            path.display()
        );
    }

    Ok(json!({
        "pipeline_tag": pipeline_tag,
        "top_n": top_n,
        "as_of": new_snap.as_of,
        "first_run": diff.first_run,
        "newcomers": diff.newcomers.iter().map(|i| json!({
            "id": i.id,
            "rank": i.rank,
            "trending_score": i.trending_score,
            "last_modified": i.last_modified,
        })).collect::<Vec<_>>(),
        "updated": diff.updated.iter().map(|i| json!({
            "id": i.id,
            "rank": i.rank,
            "trending_score": i.trending_score,
            "last_modified": i.last_modified,
            "prev_last_modified": i.prev_last_modified,
        })).collect::<Vec<_>>(),
        "unchanged_count": diff.unchanged_count,
        "snapshot_path": path.to_string_lossy(),
        "snapshot_written": written,
    }))
}

/// `model-card` 子命令：取 README 原文 + meta，README 按字节预算安全截断。
pub async fn run_model_card(model_id: &str, max_bytes: usize) -> Result<Value> {
    let client = HfClient::new()?;

    let meta = match client.model_meta(model_id).await {
        Ok(m) => m,
        Err(e) => {
            log::warn!(
                "model_card[{model_id}] meta failed: {} ({})",
                e.message,
                e.kind
            );
            return Ok(api_error_json(&e));
        }
    };
    let readme = match client.readme(model_id).await {
        Ok(r) => r,
        Err(e) => {
            log::warn!(
                "model_card[{model_id}] readme failed: {} ({})",
                e.message,
                e.kind
            );
            return Ok(api_error_json(&e));
        }
    };

    let (readme_text, truncated, found) = match readme {
        Some(full) => {
            let (t, cut) = truncate_on_char_boundary(&full, max_bytes);
            (t, cut, true)
        }
        None => (String::new(), false, false),
    };
    log::info!(
        "model_card[{model_id}] readme_found={found} truncated={truncated} num_params={:?}",
        meta.num_params,
    );

    Ok(json!({
        "id": model_id,
        "pipeline_tag": meta.pipeline_tag,
        "last_modified": meta.last_modified,
        "likes": meta.likes,
        "downloads": meta.downloads,
        "num_params": meta.num_params,
        "tags": meta.tags,
        "readme_found": found,
        "readme_truncated": truncated,
        "readme": readme_text,
    }))
}

/// 按字节预算截断字符串，保证不切断 UTF-8 字符；返回 (截断后文本, 是否发生截断)。
fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> (String, bool) {
    if s.len() <= max_bytes {
        return (s.to_string(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_no_cut_when_short() {
        let (t, cut) = truncate_on_char_boundary("hello", 100);
        assert_eq!(t, "hello");
        assert!(!cut);
    }

    #[test]
    fn truncate_respects_char_boundary() {
        // "你好" = 6 bytes (3 each); budget 4 must not split the 2nd char.
        let (t, cut) = truncate_on_char_boundary("你好", 4);
        assert_eq!(t, "你");
        assert!(cut);
    }
}
