//! `VectorStore` trait + `StoredChunk` DTO。

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::types::SearchHit;

/// 已计算向量的 chunk，由服务层组装后交给 `VectorStore::upsert`。
#[derive(Debug, Clone)]
pub struct StoredChunk {
    pub chunk_index: usize,
    pub text: String,
    pub vector: Vec<f32>,
    pub metadata: Value,
}

#[async_trait]
pub trait VectorStore: Send + Sync {
    /// upsert 语义：事务内先删 `(namespace, external_id)` 名下全部旧 chunks，
    /// 再插入新批。`chunks` 为空时等价于 delete。
    async fn upsert(
        &self,
        namespace: &str,
        external_id: &str,
        chunks: Vec<StoredChunk>,
    ) -> Result<()>;

    /// 按 query 向量在 namespace 内取 top_k；返回顺序按 score 降序。
    async fn search(
        &self,
        namespace: &str,
        query_vec: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchHit>>;

    /// 删除一整条 external_id 名下所有 chunks。
    async fn delete(&self, namespace: &str, external_id: &str) -> Result<()>;
}
