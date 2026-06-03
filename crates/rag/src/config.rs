//! 自包含 RAG 配置。
//!
//! 等价于原 zero `crates/config` 的 `[rag]` 段，但本服务独立，不依赖 zero 任何
//! crate，故在此自带一份。配置以 JSON 文件提供（与 douyin 生态约定一致），
//! 由调用方通过 `--config <绝对路径>` 传入。

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// RAG 服务配置根。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RagConfig {
    pub embedding: RagEmbeddingConfig,
    pub store: RagStoreConfig,
    /// 通用切块的字符数上限。
    pub chunk_max_chars: usize,
    /// 相邻 chunk 的字符重叠量（防止切点割断语义）。
    pub chunk_overlap_chars: usize,
}

/// OpenAI 兼容 embedding endpoint 配置。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RagEmbeddingConfig {
    /// 完整 endpoint，例如 `http://127.0.0.1:8092/v1/embeddings`。
    pub endpoint: String,
    /// 模型名（如 `BAAI/bge-m3`）。
    pub model: String,
    /// API key 所在环境变量名；为空或缺省表示无需鉴权。
    pub api_key_env: Option<String>,
    /// 输出维度。必须与 `chunks_vec` 虚表 schema 一致，启动期由
    /// `SqliteVecStore::open` 校验。bge-m3 = 1024。
    pub dim: usize,
    /// HTTP 超时秒数。
    pub timeout_secs: u64,
}

/// sqlite-vec 后端存储配置。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RagStoreConfig {
    /// 相对 workspace_root 的 sqlite 文件路径。
    pub db_path: String,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            embedding: RagEmbeddingConfig::default(),
            store: RagStoreConfig::default(),
            chunk_max_chars: 800,
            chunk_overlap_chars: 80,
        }
    }
}

impl Default for RagEmbeddingConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            model: String::new(),
            api_key_env: None,
            dim: 0,
            timeout_secs: 30,
        }
    }
}

impl Default for RagStoreConfig {
    fn default() -> Self {
        Self {
            db_path: "rag.db".to_string(),
        }
    }
}

impl RagConfig {
    /// 从 JSON 文件加载。`api_key_env` 留空时表示无鉴权。
    pub async fn load(path: &Path) -> Result<Self> {
        let raw = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("read rag config {}", path.display()))?;
        let cfg: RagConfig = serde_json::from_str(&raw)
            .with_context(|| format!("parse rag config {}", path.display()))?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_config() {
        let json = r#"{
            "embedding": {
                "endpoint": "http://127.0.0.1:8092/v1/embeddings",
                "model": "BAAI/bge-m3",
                "api_key_env": null,
                "dim": 1024,
                "timeout_secs": 30
            },
            "store": { "db_path": "rag.db" },
            "chunk_max_chars": 800,
            "chunk_overlap_chars": 80
        }"#;
        let cfg: RagConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.embedding.dim, 1024);
        assert_eq!(cfg.embedding.model, "BAAI/bge-m3");
        assert_eq!(cfg.store.db_path, "rag.db");
        assert_eq!(cfg.chunk_max_chars, 800);
    }

    #[test]
    fn defaults_fill_missing_fields() {
        let cfg: RagConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, RagConfig::default());
        assert_eq!(cfg.embedding.timeout_secs, 30);
        assert_eq!(cfg.store.db_path, "rag.db");
    }
}
