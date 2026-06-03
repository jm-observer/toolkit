//! `KnowledgeRagService`：高层编排（normalize → chunk → embed → store）。

use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::{Map, Value};

use crate::embedding::EmbeddingProvider;
use crate::normalize::{chunk_text, extract_title_hint, normalize};
use crate::store::{StoredChunk, VectorStore};
use crate::types::{IngestItem, SearchHit, SearchQuery};

const TITLE_HINT_MAX_CHARS: usize = 60;

pub struct KnowledgeRagService {
    embedding: Arc<dyn EmbeddingProvider>,
    store: Arc<dyn VectorStore>,
    chunk_max_chars: usize,
    chunk_overlap_chars: usize,
}

impl KnowledgeRagService {
    pub fn new(
        embedding: Arc<dyn EmbeddingProvider>,
        store: Arc<dyn VectorStore>,
        chunk_max_chars: usize,
        chunk_overlap_chars: usize,
    ) -> Self {
        Self {
            embedding,
            store,
            chunk_max_chars,
            chunk_overlap_chars,
        }
    }

    pub fn chunk_max_chars(&self) -> usize {
        self.chunk_max_chars
    }

    pub fn chunk_overlap_chars(&self) -> usize {
        self.chunk_overlap_chars
    }

    pub fn embedding(&self) -> &Arc<dyn EmbeddingProvider> {
        &self.embedding
    }

    pub fn store(&self) -> &Arc<dyn VectorStore> {
        &self.store
    }

    pub async fn ingest(&self, item: IngestItem) -> Result<()> {
        log::debug!(
            "rag ingest ns={} ext={} text_len={}",
            item.namespace,
            item.external_id,
            item.text.len()
        );
        // metadata 必须是 object（即使是空 {}）
        let base_meta: Map<String, Value> = match &item.metadata {
            Value::Object(map) => map.clone(),
            Value::Null => Map::new(),
            _ => {
                return Err(anyhow!(
                    "rag ingest: metadata must be JSON object (got {})",
                    item.metadata
                ));
            }
        };
        let normalized = normalize(&item.text);
        if normalized.is_empty() {
            log::debug!(
                "rag ingest skip empty after normalize ns={} ext={}",
                item.namespace,
                item.external_id
            );
            return Ok(());
        }
        let title_hint = extract_title_hint(&normalized, TITLE_HINT_MAX_CHARS);
        let chunks = chunk_text(&normalized, self.chunk_max_chars, self.chunk_overlap_chars);
        if chunks.is_empty() {
            return Ok(());
        }
        let vectors = self.embedding.embed(&chunks).await?;
        if vectors.len() != chunks.len() {
            return Err(anyhow!(
                "embedding returned {} vectors but {} chunks",
                vectors.len(),
                chunks.len()
            ));
        }
        let mut stored = Vec::with_capacity(chunks.len());
        for (i, (text, vector)) in chunks.into_iter().zip(vectors).enumerate() {
            let mut meta = base_meta.clone();
            if let Some(t) = &title_hint {
                meta.insert("title_hint".to_string(), Value::String(t.clone()));
            }
            meta.insert("chunk_index".to_string(), Value::from(i as u64));
            stored.push(StoredChunk {
                chunk_index: i,
                text,
                vector,
                metadata: Value::Object(meta),
            });
        }
        self.store
            .upsert(&item.namespace, &item.external_id, stored)
            .await
    }

    pub async fn search(&self, query: SearchQuery) -> Result<Vec<SearchHit>> {
        if query.top_k == 0 {
            return Ok(Vec::new());
        }
        let normalized_q = normalize(&query.query);
        if normalized_q.is_empty() {
            log::debug!("rag search empty query ns={}", query.namespace);
            return Ok(Vec::new());
        }
        let vectors = self.embedding.embed(&[normalized_q]).await?;
        let vector = vectors
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("embedding returned no vectors for query"))?;
        self.store
            .search(&query.namespace, &vector, query.top_k)
            .await
    }

    pub async fn delete(&self, namespace: &str, external_id: &str) -> Result<()> {
        self.store.delete(namespace, external_id).await
    }
}
