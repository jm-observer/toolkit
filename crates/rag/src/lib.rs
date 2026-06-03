//! douyin 知识库的语义检索（RAG）服务。
//!
//! 把已落盘的知识条目（`knowledge/douyin/<抖音号>/transcripts/<aweme_id>.md`）
//! 向量化进 sqlite-vec，对外以 CLI（`rag ingest` / `rag search`）和 HTTP
//! （`rag serve`）暴露检索。
//!
//! 由 zero `crates/knowledge-rag` 迁出并去耦合（自带 [`config`]，不依赖 zero）。

pub mod config;
pub mod embedding;
pub mod embedding_http;
pub mod ingest;
pub mod normalize;
pub mod serve;
pub mod service;
pub mod store;
pub mod store_sqlite;
pub mod types;

pub use config::{RagConfig, RagEmbeddingConfig, RagStoreConfig};
pub use embedding::EmbeddingProvider;
pub use embedding_http::OpenAiCompatEmbedding;
pub use ingest::{ingest_douyin_knowledge, IngestStats};
pub use service::KnowledgeRagService;
pub use store::{StoredChunk, VectorStore};
pub use store_sqlite::SqliteVecStore;
pub use types::{IngestItem, SearchHit, SearchQuery};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

/// 默认 namespace。
pub const DOUYIN_NAMESPACE: &str = "douyin";

/// 解析 workspace 根：显式 `explicit` 优先，否则 `ZERO_WORKSPACE` 环境变量，
/// 再否则 `$HOME/.config/zero`（与 douyin 约定一致）。
pub fn resolve_workspace(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Ok(p) = std::env::var("ZERO_WORKSPACE") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("resolve workspace: neither --workspace, ZERO_WORKSPACE, nor HOME set")?;
    Ok(PathBuf::from(home).join(".config").join("zero"))
}

/// 用配置构建 [`KnowledgeRagService`]（embedding HTTP 客户端 + sqlite-vec store）。
pub async fn build_service(cfg: &RagConfig, workspace_root: &Path) -> Result<KnowledgeRagService> {
    let embedding =
        OpenAiCompatEmbedding::from_config(&cfg.embedding).context("build embedding provider")?;
    let dim = embedding.dim();
    let store = SqliteVecStore::open(workspace_root, &cfg.store, dim)
        .await
        .context("open sqlite-vec store")?;
    Ok(KnowledgeRagService::new(
        Arc::new(embedding),
        Arc::new(store),
        cfg.chunk_max_chars,
        cfg.chunk_overlap_chars,
    ))
}
