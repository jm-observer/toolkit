//! OpenAI 兼容 `/v1/embeddings` HTTP 客户端。
//!
//! - 请求体：`{ "model": ..., "input": [..], "encoding_format": "float" }`
//! - 响应体：`{ "data": [{ "index": N, "embedding": [..] }, ..] }`，按 `index` 升序排列
//! - 不做内部 retry；失败由调用方下轮重试覆盖
//! - 空输入直接返回 `Ok(vec![])`，不发请求

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::RagEmbeddingConfig;
use crate::embedding::EmbeddingProvider;

#[derive(Debug)]
pub struct OpenAiCompatEmbedding {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    /// Set to `Some` when `RagEmbeddingConfig::api_key_env` resolved successfully.
    api_key: Option<String>,
    dim: usize,
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
    encoding_format: &'a str,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

impl OpenAiCompatEmbedding {
    pub fn from_config(cfg: &RagEmbeddingConfig) -> Result<Self> {
        if cfg.endpoint.is_empty() {
            return Err(anyhow!("embedding endpoint empty"));
        }
        if cfg.model.is_empty() {
            return Err(anyhow!("embedding model empty"));
        }
        if cfg.dim == 0 {
            return Err(anyhow!("embedding dim must be > 0"));
        }
        let api_key = match cfg.api_key_env.as_deref() {
            Some(var_name) if !var_name.is_empty() => Some(
                std::env::var(var_name)
                    .map_err(|_| anyhow!("embedding api key env var {var_name} not set"))?,
            ),
            _ => None,
        };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_secs))
            .build()
            .context("build reqwest client for embedding")?;
        Ok(Self {
            client,
            endpoint: cfg.endpoint.clone(),
            model: cfg.model.clone(),
            api_key,
            dim: cfg.dim,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatEmbedding {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let body = EmbeddingRequest {
            model: &self.model,
            input: texts,
            encoding_format: "float",
        };
        let mut req = self
            .client
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .json(&body);
        if let Some(key) = self.api_key.as_deref() {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                anyhow!("embedding request timeout: endpoint={}", self.endpoint)
            } else if e.is_connect() {
                anyhow!(
                    "embedding connect failed: endpoint={} err={}",
                    self.endpoint,
                    e
                )
            } else {
                anyhow!(
                    "embedding request failed: endpoint={} err={}",
                    self.endpoint,
                    e
                )
            }
        })?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(200).collect();
            return Err(anyhow!(
                "embedding http {} from {}: {}",
                status.as_u16(),
                self.endpoint,
                snippet
            ));
        }
        let raw = resp.text().await.context("read embedding response body")?;
        let parsed: EmbeddingResponse = serde_json::from_str(&raw).map_err(|e| {
            let snippet: String = raw.chars().take(200).collect();
            anyhow!(
                "embedding response parse failed: endpoint={} err={} body={}",
                self.endpoint,
                e,
                snippet
            )
        })?;
        if parsed.data.len() != texts.len() {
            return Err(anyhow!(
                "embedding count mismatch: expected={} actual={} endpoint={}",
                texts.len(),
                parsed.data.len(),
                self.endpoint
            ));
        }
        let mut items = parsed.data;
        items.sort_by_key(|it| it.index);
        let mut out = Vec::with_capacity(items.len());
        for (pos, item) in items.into_iter().enumerate() {
            if item.embedding.len() != self.dim {
                return Err(anyhow!(
                    "embedding dim mismatch at item {}: expected={} actual={} endpoint={}",
                    pos,
                    self.dim,
                    item.embedding.len(),
                    self.endpoint
                ));
            }
            out.push(item.embedding);
        }
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }
}
