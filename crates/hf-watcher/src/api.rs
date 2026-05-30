//! HuggingFace HTTP API 薄封装：trending 列表、模型 meta、README 原文。
//!
//! 所有网络/HTTP 错误统一映射为 [`ApiError`]，由命令层转成结构化 JSON，
//! 避免进程级 panic / 非零退出干扰调用方解析。

use anyhow::{Context, Result};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::time::Duration;

const API_BASE: &str = "https://huggingface.co";

/// 可恢复的业务错误，命令层据此输出 `{error, error_kind}`。
#[derive(Debug, Clone)]
pub struct ApiError {
    pub kind: &'static str,
    pub message: String,
}

impl ApiError {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// trending 列表中的单条模型（已按指定 pipeline_tag 过滤）。
#[derive(Debug, Clone)]
pub struct TrendingEntry {
    pub id: String,
    pub trending_score: f64,
    pub last_modified: String,
    pub likes: i64,
    pub downloads: i64,
}

/// 单模型补充 meta（README 命令使用）。
#[derive(Debug, Clone)]
pub struct ModelMeta {
    pub last_modified: Option<String>,
    pub likes: Option<i64>,
    pub downloads: Option<i64>,
    pub pipeline_tag: Option<String>,
    pub tags: Vec<String>,
    /// 参数量（取 `safetensors.total`），量化/GGUF 仓常缺，缺失为 None。
    pub num_params: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawTrending {
    id: String,
    #[serde(rename = "trendingScore", default)]
    trending_score: f64,
    #[serde(rename = "lastModified", default)]
    last_modified: Option<String>,
    #[serde(default)]
    likes: i64,
    #[serde(default)]
    downloads: i64,
}

#[derive(Debug, Deserialize)]
struct RawMeta {
    #[serde(rename = "lastModified")]
    last_modified: Option<String>,
    likes: Option<i64>,
    downloads: Option<i64>,
    #[serde(rename = "pipeline_tag")]
    pipeline_tag: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    safetensors: Option<RawSafetensors>,
}

#[derive(Debug, Deserialize)]
struct RawSafetensors {
    total: Option<u64>,
}

pub struct HfClient {
    client: Client,
    token: Option<String>,
}

impl HfClient {
    pub fn new() -> Result<Self> {
        // reqwest 默认尊重标准代理环境变量（HTTP_PROXY / HTTPS_PROXY / ALL_PROXY /
        // NO_PROXY），无需在代码里显式配置——部署侧（如 G10）通过这些标准变量把
        // huggingface.co 的流量导向本地代理，并用 NO_PROXY 排除本机 / 局域网端点。
        // 连接级超时收紧，便于在网络坏窗口下快速失败后由 send_with_retry 重试。
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .user_agent("zero-hf-watcher")
            .build()
            .context("构造 HTTP 客户端失败")?;
        let token = std::env::var("HF_TOKEN").ok().filter(|t| !t.is_empty());
        Ok(Self { client, token })
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    /// 发送请求，对连接级失败（首连抖动 / 超时）重试。
    ///
    /// G10 → huggingface.co 的首次连接常抖动（DNS/TLS），随后立即可用；
    /// 每个工具调用是独立短进程，无重试则单次抖动即整类失败。这里以
    /// 递增退避重试，仅覆盖网络层错误；HTTP 状态错误由 [`ensure_success`] 处理。
    async fn send_with_retry<F>(
        &self,
        build: F,
        what: &str,
    ) -> std::result::Result<reqwest::Response, ApiError>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        const MAX_ATTEMPTS: u32 = 6;
        let mut last_err = String::new();
        for attempt in 1..=MAX_ATTEMPTS {
            match self.authed(build()).send().await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    last_err = e.to_string();
                    log::debug!("{what} 第 {attempt}/{MAX_ATTEMPTS} 次请求失败: {last_err}");
                    if attempt < MAX_ATTEMPTS {
                        let backoff = Duration::from_millis(400 * u64::from(attempt));
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }
        Err(ApiError::new(
            "network",
            format!("{what} 请求失败（重试 {MAX_ATTEMPTS} 次后）: {last_err}"),
        ))
    }

    /// 取某 pipeline_tag 的 trending top-N（对应网页 `?sort=trending`）。
    pub async fn trending(
        &self,
        pipeline_tag: &str,
        top_n: usize,
    ) -> std::result::Result<Vec<TrendingEntry>, ApiError> {
        let url = format!("{API_BASE}/api/models");
        let mut query: Vec<(&str, String)> = vec![
            ("pipeline_tag", pipeline_tag.to_string()),
            ("sort", "trendingScore".to_string()),
            ("direction", "-1".to_string()),
            ("limit", top_n.to_string()),
        ];
        for field in ["lastModified", "trendingScore", "likes", "downloads"] {
            query.push(("expand[]", field.to_string()));
        }

        let resp = self
            .send_with_retry(|| self.client.get(&url).query(&query), "trending")
            .await?;
        let resp = ensure_success(resp, "trending").await?;
        let raw: Vec<RawTrending> = resp
            .json()
            .await
            .map_err(|e| ApiError::new("parse", format!("解析 trending JSON 失败: {e}")))?;

        Ok(raw
            .into_iter()
            .map(|r| TrendingEntry {
                id: r.id,
                trending_score: r.trending_score,
                last_modified: r.last_modified.unwrap_or_default(),
                likes: r.likes,
                downloads: r.downloads,
            })
            .collect())
    }

    /// 取单模型 meta（参数量、likes、tags 等）。
    pub async fn model_meta(&self, id: &str) -> std::result::Result<ModelMeta, ApiError> {
        let url = format!("{API_BASE}/api/models/{id}");
        let resp = self
            .send_with_retry(|| self.client.get(&url), "model_meta")
            .await?;
        let resp = ensure_success(resp, "model_meta").await?;
        let raw: RawMeta = resp
            .json()
            .await
            .map_err(|e| ApiError::new("parse", format!("解析 model meta JSON 失败: {e}")))?;
        Ok(ModelMeta {
            last_modified: raw.last_modified,
            likes: raw.likes,
            downloads: raw.downloads,
            pipeline_tag: raw.pipeline_tag,
            tags: raw.tags,
            num_params: raw.safetensors.and_then(|s| s.total),
        })
    }

    /// 取 README 原文，依次尝试 `main` 与 `master` 分支；两者均 404 返回 None。
    pub async fn readme(&self, id: &str) -> std::result::Result<Option<String>, ApiError> {
        for branch in ["main", "master"] {
            let url = format!("{API_BASE}/{id}/raw/{branch}/README.md");
            let resp = self
                .send_with_retry(|| self.client.get(&url), "readme")
                .await?;
            if resp.status() == StatusCode::NOT_FOUND {
                continue;
            }
            let resp = ensure_success(resp, "readme").await?;
            let text = resp
                .text()
                .await
                .map_err(|e| ApiError::new("parse", format!("读取 README 文本失败: {e}")))?;
            return Ok(Some(text));
        }
        Ok(None)
    }
}

/// 非 2xx 状态统一转 [`ApiError`]：404 → not_found，其余 → http。
async fn ensure_success(
    resp: reqwest::Response,
    what: &str,
) -> std::result::Result<reqwest::Response, ApiError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    if status == StatusCode::NOT_FOUND {
        return Err(ApiError::new(
            "not_found",
            format!("{what} 返回 404（资源不存在）"),
        ));
    }
    Err(ApiError::new("http", format!("{what} 返回 HTTP {status}")))
}
