//! 每类型一份的 trending 快照：读 / 写 / 与本次结果对比。
//!
//! 快照落在 `<snapshot_dir>/snapshot-<sanitized-tag>.json`。`snapshot_dir` 由
//! 调用方（zero agent）传入，工具不做默认值/回退。
//! 对比产出 newcomers（新进榜）与 updated（在榜且 lastModified 变新）。

use crate::api::TrendingEntry;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub as_of: String,
    pub pipeline_tag: String,
    pub top_n: usize,
    pub entries: Vec<SnapEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapEntry {
    pub id: String,
    pub rank: usize,
    pub trending_score: f64,
    pub last_modified: String,
}

/// 对比结果。`first_run` 为 true 时仅做基线播种，不产出 newcomers/updated。
#[derive(Debug, Default)]
pub struct DiffResult {
    pub newcomers: Vec<DiffItem>,
    pub updated: Vec<DiffItem>,
    pub unchanged_count: usize,
    pub first_run: bool,
}

#[derive(Debug, Clone)]
pub struct DiffItem {
    pub id: String,
    pub rank: usize,
    pub trending_score: f64,
    pub last_modified: String,
    /// updated 专用：上一次记录的 last_modified。
    pub prev_last_modified: Option<String>,
}

/// 把 pipeline_tag 里的 `/` 等替换为 `_`，用作文件名。
fn sanitize_tag(tag: &str) -> String {
    tag.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

pub fn snapshot_path(snapshot_dir: &Path, pipeline_tag: &str) -> PathBuf {
    snapshot_dir.join(format!("snapshot-{}.json", sanitize_tag(pipeline_tag)))
}

pub fn load(path: &Path) -> Result<Option<Snapshot>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取快照失败: {}", path.display()))?;
    let snap = serde_json::from_str(&text)
        .with_context(|| format!("解析快照 JSON 失败: {}", path.display()))?;
    Ok(Some(snap))
}

pub fn save(path: &Path, snap: &Snapshot) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建快照目录失败: {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(snap).context("序列化快照失败")?;
    std::fs::write(path, text).with_context(|| format!("写入快照失败: {}", path.display()))?;
    Ok(())
}

/// 由本次 trending 结果构造新快照。
pub fn build_snapshot(
    pipeline_tag: &str,
    top_n: usize,
    as_of: String,
    entries: &[TrendingEntry],
) -> Snapshot {
    Snapshot {
        as_of,
        pipeline_tag: pipeline_tag.to_string(),
        top_n,
        entries: entries
            .iter()
            .enumerate()
            .map(|(i, e)| SnapEntry {
                id: e.id.clone(),
                rank: i + 1,
                trending_score: e.trending_score,
                last_modified: e.last_modified.clone(),
            })
            .collect(),
    }
}

/// 对比上次快照与本次结果。`prev` 为 None 时标记为首跑。
pub fn diff(prev: Option<&Snapshot>, current: &[TrendingEntry]) -> DiffResult {
    let Some(prev) = prev else {
        return DiffResult {
            first_run: true,
            ..Default::default()
        };
    };

    let mut result = DiffResult::default();
    for (idx, entry) in current.iter().enumerate() {
        let rank = idx + 1;
        match prev.entries.iter().find(|p| p.id == entry.id) {
            None => result.newcomers.push(DiffItem {
                id: entry.id.clone(),
                rank,
                trending_score: entry.trending_score,
                last_modified: entry.last_modified.clone(),
                prev_last_modified: None,
            }),
            Some(prev_entry) => {
                if is_newer(&entry.last_modified, &prev_entry.last_modified) {
                    result.updated.push(DiffItem {
                        id: entry.id.clone(),
                        rank,
                        trending_score: entry.trending_score,
                        last_modified: entry.last_modified.clone(),
                        prev_last_modified: Some(prev_entry.last_modified.clone()),
                    });
                } else {
                    result.unchanged_count += 1;
                }
            }
        }
    }
    result
}

/// 比较两个 RFC3339 时间串，`cur` 是否晚于 `prev`。
/// 优先按 chrono 解析；解析失败时回退字符串比较（HF 返回格式一致，安全）。
fn is_newer(cur: &str, prev: &str) -> bool {
    use chrono::{DateTime, Utc};
    match (cur.parse::<DateTime<Utc>>(), prev.parse::<DateTime<Utc>>()) {
        (Ok(a), Ok(b)) => a > b,
        _ => cur > prev,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, lm: &str) -> TrendingEntry {
        TrendingEntry {
            id: id.to_string(),
            trending_score: 1.0,
            last_modified: lm.to_string(),
            likes: 0,
            downloads: 0,
        }
    }

    fn snap(entries: &[(&str, &str)]) -> Snapshot {
        Snapshot {
            as_of: "2026-05-20T00:00:00+00:00".to_string(),
            pipeline_tag: "text-to-speech".to_string(),
            top_n: 5,
            entries: entries
                .iter()
                .enumerate()
                .map(|(i, (id, lm))| SnapEntry {
                    id: id.to_string(),
                    rank: i + 1,
                    trending_score: 1.0,
                    last_modified: lm.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn first_run_seeds_only() {
        let cur = vec![entry("a/x", "2026-05-20T00:00:00.000Z")];
        let d = diff(None, &cur);
        assert!(d.first_run);
        assert!(d.newcomers.is_empty());
        assert!(d.updated.is_empty());
    }

    #[test]
    fn detects_newcomer() {
        let prev = snap(&[("a/x", "2026-05-10T00:00:00.000Z")]);
        let cur = vec![
            entry("a/x", "2026-05-10T00:00:00.000Z"),
            entry("b/y", "2026-05-19T00:00:00.000Z"),
        ];
        let d = diff(Some(&prev), &cur);
        assert_eq!(d.newcomers.len(), 1);
        assert_eq!(d.newcomers[0].id, "b/y");
        assert_eq!(d.newcomers[0].rank, 2);
        assert_eq!(d.unchanged_count, 1);
    }

    #[test]
    fn detects_updated_checkpoint() {
        let prev = snap(&[("a/x", "2026-05-10T00:00:00.000Z")]);
        let cur = vec![entry("a/x", "2026-05-21T00:00:00.000Z")];
        let d = diff(Some(&prev), &cur);
        assert_eq!(d.updated.len(), 1);
        assert_eq!(
            d.updated[0].prev_last_modified.as_deref(),
            Some("2026-05-10T00:00:00.000Z")
        );
        assert_eq!(d.newcomers.len(), 0);
    }

    #[test]
    fn unchanged_when_same_last_modified() {
        let prev = snap(&[("a/x", "2026-05-10T00:00:00.000Z")]);
        let cur = vec![entry("a/x", "2026-05-10T00:00:00.000Z")];
        let d = diff(Some(&prev), &cur);
        assert_eq!(d.unchanged_count, 1);
        assert!(d.updated.is_empty());
        assert!(d.newcomers.is_empty());
    }

    #[test]
    fn sanitize_tag_replaces_non_alnum() {
        assert_eq!(sanitize_tag("image-text-to-text"), "image_text_to_text");
        assert_eq!(sanitize_tag("any-to-any"), "any_to_any");
    }

    #[test]
    fn is_newer_handles_iso() {
        assert!(is_newer(
            "2026-05-21T00:00:00.000Z",
            "2026-05-10T00:00:00.000Z"
        ));
        assert!(!is_newer(
            "2026-05-10T00:00:00.000Z",
            "2026-05-21T00:00:00.000Z"
        ));
    }
}
