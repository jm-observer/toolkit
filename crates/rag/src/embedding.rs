//! `EmbeddingProvider` trait。

use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// 批量向量化，返回顺序与输入对齐。空输入应直接返回空 vec，不发请求。
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// 输出维度。运行期与 `VectorStore` schema 校验需此值。
    fn dim(&self) -> usize;
}
