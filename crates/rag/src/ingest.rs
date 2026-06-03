//! douyin 知识包录入：扫描 `<workspace>/knowledge/douyin/<抖音号>/transcripts/*.md`，
//! 逐文件 upsert 进向量库。
//!
//! 映射：
//! - `namespace` = 调用方给定（默认 `douyin`）
//! - `external_id` = 文件名去扩展（即 `aweme_id`）
//! - `text` = 整个 md 内容（service 内部 normalize + chunk）
//! - `metadata` = `{ author_id, source_path, mtime_secs }`
//!
//! upsert 幂等，重复全量扫描安全。失败逐条计数并继续，不中断整体。

use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use serde_json::json;

use crate::service::KnowledgeRagService;
use crate::types::IngestItem;

/// 录入统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct IngestStats {
    pub ingested: usize,
    pub skipped: usize,
    pub failed: usize,
}

/// 全量扫描 douyin 知识包目录并 ingest。
///
/// `workspace_root` 下约定路径：`knowledge/douyin/<author_id>/transcripts/*.md`。
/// 目录缺失视为零条（返回全 0 统计，不报错）。
pub async fn ingest_douyin_knowledge(
    service: &KnowledgeRagService,
    workspace_root: &Path,
    namespace: &str,
) -> Result<IngestStats> {
    let root = workspace_root.join("knowledge").join("douyin");
    let mut stats = IngestStats::default();
    if !root.is_dir() {
        log::warn!(
            "rag ingest: douyin knowledge dir absent: {}",
            root.display()
        );
        return Ok(stats);
    }

    let mut authors = tokio::fs::read_dir(&root)
        .await
        .with_context(|| format!("read dir {}", root.display()))?;
    while let Some(author_entry) = authors.next_entry().await? {
        let author_path = author_entry.path();
        if !author_path.is_dir() {
            continue;
        }
        let author_id = author_entry.file_name().to_string_lossy().to_string();
        let transcripts = author_path.join("transcripts");
        if !transcripts.is_dir() {
            continue;
        }
        ingest_author_dir(
            service,
            workspace_root,
            namespace,
            &author_id,
            &transcripts,
            &mut stats,
        )
        .await?;
    }
    log::info!(
        "rag ingest done: ingested={} skipped={} failed={}",
        stats.ingested,
        stats.skipped,
        stats.failed
    );
    Ok(stats)
}

async fn ingest_author_dir(
    service: &KnowledgeRagService,
    workspace_root: &Path,
    namespace: &str,
    author_id: &str,
    transcripts: &Path,
    stats: &mut IngestStats,
) -> Result<()> {
    let mut files = tokio::fs::read_dir(transcripts)
        .await
        .with_context(|| format!("read dir {}", transcripts.display()))?;
    while let Some(file_entry) = files.next_entry().await? {
        let path = file_entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let external_id = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                stats.skipped += 1;
                continue;
            }
        };
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(t) => t,
            Err(e) => {
                log::warn!("rag ingest read failed {}: {}", path.display(), e);
                stats.failed += 1;
                continue;
            }
        };
        if text.trim().is_empty() {
            stats.skipped += 1;
            continue;
        }
        let mtime_secs = file_mtime_secs(&path).await;
        let source_path = path
            .strip_prefix(workspace_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = json!({
            "author_id": author_id,
            "source_path": source_path,
            "mtime_secs": mtime_secs,
        });
        let item = IngestItem {
            external_id,
            namespace: namespace.to_string(),
            text,
            metadata,
        };
        match service.ingest(item).await {
            Ok(()) => stats.ingested += 1,
            Err(e) => {
                log::warn!("rag ingest failed {}: {}", path.display(), e);
                stats.failed += 1;
            }
        }
    }
    Ok(())
}

async fn file_mtime_secs(path: &Path) -> u64 {
    let Ok(meta) = tokio::fs::metadata(path).await else {
        return 0;
    };
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
