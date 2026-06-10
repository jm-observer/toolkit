//! AudioForge 的上游 TTS 客户端：直接调 `TTS_BASE_URL/tts`（复用 Phase 1 的配置约定）。
//!
//! 与 `routes::audio` 的「透明代理」不同——这里是任务内的逐句生成：带重试、带 trace
//! 子 span、返回 WAV bytes 供落盘。请求体形态与上游 CosyVoice2 `/tts` 对齐：
//! `{text, voice_id, ...tts_params}`（tts_params 平铺进 body，如 speed / instruct）。

use anyhow::{anyhow, bail, Context, Result};
use custom_utils::trace::{self, SpanScope, SpanStatus, TraceContext};
use serde_json::{json, Value};
use std::time::Duration;

/// 单句 TTS 生成可能 10s+，给足超时。
const TTS_TIMEOUT: Duration = Duration::from_secs(180);
/// 单句最大重试次数（含首次）。
const MAX_ATTEMPTS: usize = 3;

/// 读上游 TTS base URL（与 `routes::audio` 同约定）。未配置返回 None。
pub fn tts_base_url() -> Option<String> {
    std::env::var("TTS_BASE_URL")
        .ok()
        .map(|s| s.trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

pub struct TtsClient {
    http: reqwest::Client,
    base: String,
}

impl TtsClient {
    pub fn new(base: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(TTS_TIMEOUT)
            .build()
            .context("构建 TTS HTTP client")?;
        Ok(Self { http, base })
    }

    /// 生成单句音频，带重试。`parent` 为顶层任务 span 身份（trace 关闭时为 None）。
    pub async fn synthesize_traced(
        &self,
        text: &str,
        voice_id: &str,
        tts_params: &Value,
        parent: Option<&TraceContext>,
    ) -> Result<Vec<u8>> {
        if text.trim().is_empty() {
            bail!("句子文本为空");
        }
        let scope = match (trace::enabled(), parent) {
            (true, Some(p)) => {
                let scope = SpanScope::new(p.child(), "tts_one").with_summary(json!({
                    "voice_id": voice_id,
                    "text_len": text.chars().count(),
                }));
                scope.emit_start();
                Some(scope)
            }
            _ => None,
        };

        let result = self.synthesize_with_retry(text, voice_id, tts_params).await;

        match &result {
            Ok(bytes) => {
                if let Some(s) = scope {
                    s.emit_end(
                        Some(format!("{} bytes", bytes.len())),
                        SpanStatus::Ok,
                        Some(json!({ "resp_bytes": bytes.len() })),
                    );
                }
            }
            Err(e) => {
                if let Some(s) = scope {
                    s.emit_end(None, SpanStatus::Error(format!("{e:#}")), None);
                }
            }
        }
        result
    }

    async fn synthesize_with_retry(
        &self,
        text: &str,
        voice_id: &str,
        tts_params: &Value,
    ) -> Result<Vec<u8>> {
        let mut last_err = None;
        for attempt in 1..=MAX_ATTEMPTS {
            match self.synthesize_once(text, voice_id, tts_params).await {
                Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
                Ok(_) => last_err = Some(anyhow!("上游返回空音频")),
                Err(e) => last_err = Some(e),
            }
            if attempt < MAX_ATTEMPTS {
                // 指数退避：0.5s, 1s, ...
                let backoff = Duration::from_millis(500 * (1 << (attempt - 1)) as u64);
                tokio::time::sleep(backoff).await;
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("TTS 调用失败（未知）")))
    }

    async fn synthesize_once(
        &self,
        text: &str,
        voice_id: &str,
        tts_params: &Value,
    ) -> Result<Vec<u8>> {
        // body = {text, voice_id} ∪ tts_params（params 平铺，如 speed / instruct）。
        let mut body = json!({ "text": text, "voice_id": voice_id });
        if let Some(extra) = tts_params.as_object() {
            let obj = body.as_object_mut().expect("body is object");
            for (k, v) in extra {
                obj.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }

        let url = format!("{}/tts", self.base);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("调上游 TTS /tts")?;
        let status = resp.status();
        if !status.is_success() {
            let txt = resp.text().await.unwrap_or_default();
            bail!(
                "TTS {status}: {}",
                txt.chars().take(300).collect::<String>()
            );
        }
        let bytes = resp.bytes().await.context("读 TTS 响应体")?;
        Ok(bytes.to_vec())
    }
}
