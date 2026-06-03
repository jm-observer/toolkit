//! DTO 定义。本模块刻意不依赖任何后端 crate，使 trait 实现方可独立替换。

use serde::Serialize;
use serde_json::Value;

/// 录入请求：调用方提供稳定 `external_id` 作为 upsert 键，
/// 同一 (namespace, external_id) 的旧 chunks 会被整段替换。
#[derive(Debug, Clone)]
pub struct IngestItem {
    /// 调用方稳定唯一 ID（如 douyin `aweme_id`、md 相对路径）。
    pub external_id: String,
    /// 隔离命名空间，例如 `douyin` / `english-coach` / `general`。
    pub namespace: String,
    /// 原始文本；服务层负责通用整理（normalize + chunk），
    /// 领域特化清洗由调用方完成。
    pub text: String,
    /// 任意 JSON metadata，回填到 `SearchHit.metadata`。
    pub metadata: Value,
}

/// 检索单条命中。
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub external_id: String,
    pub chunk_index: usize,
    pub text: String,
    /// 相似度分数（实现层定义；sqlite-vec 余弦距离时 `score = 1 - distance`）。
    pub score: f32,
    pub metadata: Value,
}

/// 检索请求参数。
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub namespace: String,
    pub query: String,
    pub top_k: usize,
}
