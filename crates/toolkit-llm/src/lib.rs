//! toolkit-llm：统一的 OpenAI 兼容 chat completions 客户端。
//!
//! 把原先散在 `douyin::refine` 里的「连接配置 + 单次调用 + 指数退避重试 + 响应解析」
//! 抽成可复用底座。任何需要调大模型的内部 crate（douyin 整理、对话总结、未来功能）都走
//! [`LlmClient`]，不再各自拼 HTTP。
//!
//! ## 配置来源
//! [`LlmConfig`] 既可由调用方从持久化配置（toolkit.db）装配，也可用 [`LlmConfig::from_env`]
//! 从环境变量兜底：`LLM_BASE_URL`（OpenAI 兼容 base，如 `http://gb10:8000/v1`）、`LLM_MODEL`、
//! 可选 `LLM_API_KEY`。
//!
//! ## 提示词
//! 本 crate **不持有任何提示词**——提示词由各功能层（含 DB 可配）决定后，作为消息文本传入。

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 默认单次请求超时。TTS/整理类调用可能 10s+，给足余量。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);
/// 单次调用最大尝试次数（含首次）。
const MAX_ATTEMPTS: usize = 3;

/// LLM 连接配置。可由调用方手工装配，或 [`from_env`](Self::from_env) 从环境变量兜底。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmConfig {
    /// OpenAI 兼容 base（末尾斜杠已规整去除），如 `http://gb10:8000/v1`。
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl LlmConfig {
    /// 显式构造（base_url 自动去尾部斜杠，空 api_key 归一为 None）。
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            api_key: api_key.filter(|s| !s.trim().is_empty()),
        }
    }

    /// 从环境变量装配；缺 `LLM_BASE_URL` / `LLM_MODEL` 时明确报错。
    pub fn from_env() -> Result<Self> {
        let base_url = std::env::var("LLM_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .context("未配置 LLM_BASE_URL（OpenAI 兼容 base，如 http://gb10:8000/v1）")?;
        let model = std::env::var("LLM_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .context("未配置 LLM_MODEL")?;
        let api_key = std::env::var("LLM_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty());
        Ok(Self::new(base_url, model, api_key))
    }
}

/// 提示词文本短哈希（sm3 前 8 字节 → 16 hex）。提示词改了哈希就变，可识别旧产物 / 检测是否
/// 已偏离内置默认。与原 `douyin::refine::prompt_hash` 算法一致，保证既有落盘元信息可比对。
pub fn prompt_hash(text: &str) -> String {
    use sm3::{Digest, Sm3};
    let mut h = Sm3::new();
    h.update(text.as_bytes());
    let out = h.finalize();
    out.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// 一条聊天消息。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

/// 统一 LLM 客户端：持连接配置 + 复用的 reqwest client。
#[derive(Clone)]
pub struct LlmClient {
    cfg: LlmConfig,
    http: reqwest::Client,
    temperature: f32,
}

impl LlmClient {
    /// 用默认超时 + 默认温度 0.2 构造。
    pub fn new(cfg: LlmConfig) -> Result<Self> {
        Self::with_timeout(cfg, DEFAULT_TIMEOUT)
    }

    /// 指定单次请求超时构造。
    pub fn with_timeout(cfg: LlmConfig, timeout: Duration) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .context("构建 LLM HTTP client")?;
        Ok(Self {
            cfg,
            http,
            temperature: 0.2,
        })
    }

    /// 覆盖采样温度。
    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = t;
        self
    }

    /// 当前模型名（落产物元信息用）。
    pub fn model(&self) -> &str {
        &self.cfg.model
    }

    /// 单 user 消息便捷调用。
    pub async fn complete(&self, prompt: &str) -> Result<String> {
        self.chat(&[Message::user(prompt)]).await
    }

    /// 通用 chat completions：失败按指数退避重试，返回 assistant 文本。
    pub async fn chat(&self, messages: &[Message]) -> Result<String> {
        let mut last_err = None;
        for attempt in 1..=MAX_ATTEMPTS {
            match self.chat_once(messages).await {
                Ok(text) if !text.trim().is_empty() => return Ok(text),
                Ok(_) => last_err = Some(anyhow!("LLM 返回空文本")),
                Err(e) => last_err = Some(e),
            }
            if attempt < MAX_ATTEMPTS {
                // 指数退避：0.5s, 1s, ...
                let backoff = Duration::from_millis(500 * (1 << (attempt - 1)) as u64);
                tokio::time::sleep(backoff).await;
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("LLM 调用失败（未知）")))
    }

    async fn chat_once(&self, messages: &[Message]) -> Result<String> {
        let url = format!("{}/chat/completions", self.cfg.base_url);
        let body = serde_json::json!({
            "model": self.cfg.model,
            "messages": messages,
            "temperature": self.temperature,
            "stream": false,
        });
        let mut req = self.http.post(&url).json(&body);
        if let Some(key) = &self.cfg.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.context("调 LLM chat completions")?;
        let status = resp.status();
        let text = resp.text().await.context("读 LLM 响应体")?;
        if !status.is_success() {
            bail!(
                "LLM {status}: {}",
                text.chars().take(300).collect::<String>()
            );
        }
        let parsed: ChatResponse = serde_json::from_str(&text).context("解析 LLM 响应 JSON")?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| anyhow!("LLM 响应无 choices"))?;
        Ok(content.trim().to_string())
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_trims_slash_and_empty_key() {
        let c = LlmConfig::new("http://x/v1/", "m", Some("  ".to_string()));
        assert_eq!(c.base_url, "http://x/v1");
        assert_eq!(c.api_key, None);
        let c2 = LlmConfig::new("http://x", "m", Some("k".into()));
        assert_eq!(c2.api_key.as_deref(), Some("k"));
    }

    #[test]
    fn chat_response_parses() {
        let raw = r#"{"choices":[{"message":{"role":"assistant","content":"hello"}}]}"#;
        let p: ChatResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(p.choices[0].message.content, "hello");
    }

    #[test]
    fn message_constructors() {
        assert_eq!(Message::system("a").role, "system");
        assert_eq!(Message::user("b").role, "user");
        assert_eq!(Message::assistant("c").role, "assistant");
    }
}
