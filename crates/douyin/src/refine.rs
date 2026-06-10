//! Phase 2：LLM 整理文本（TextRefine）。
//!
//! 输入 ASR 原文 → 调 GB10 vLLM（OpenAI 兼容 chat completions）做纠错 / 去口语水词 /
//! 分段 / 小结 → 产出「整理稿」。整理稿落 `douyin/refined/<aweme_id>.json`，与
//! `douyin/transcripts/<aweme_id>.json`（ASR 原文）并列；`knowledge::run_publish_knowledge`
//! 读取它并写进 knowledge md 的「整理稿」栏，从而被 rag ingest 索引（优先整理稿）。
//!
//! ## 配置（环境变量）
//! - `LLM_BASE_URL`：OpenAI 兼容 base，如 `http://gb10:8000/v1`（必填，未配置时整理任务提交即报错）。
//! - `LLM_MODEL`：模型名（必填）。
//! - `LLM_API_KEY`：可选 Bearer token（vLLM 默认无鉴权时留空）。
//!
//! ## prompt 管理
//! 整理 prompt 内置在 `refine_prompt.md`（随 crate 编译）。每条整理稿元信息记录
//! `prompt_version`（人工语义版本）与 `prompt_hash`（prompt 文本的 sm3 短哈希），
//! 便于迭代 prompt 后识别哪些条目是旧 prompt 产物、按需重跑对比。
//!
//! ## 重试 / 容错
//! 单条 LLM 调用失败按 `MAX_ATTEMPTS` 重试（指数退避）；整批中单条最终失败不拖垮其余，
//! 失败条目进任务 output 的 `failed[]`。

use anyhow::{anyhow, bail, Context, Result};
use custom_utils::trace::{self, SpanStatus, TraceContext};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 内置整理 prompt（随 crate 编译）。`{TRANSCRIPT}` 占位符在调用时替换为 ASR 原文。
pub const REFINE_PROMPT: &str = include_str!("refine_prompt.md");

/// prompt 人工语义版本。迭代 prompt 文案时同步 bump，便于按版本筛选 / 重跑。
pub const PROMPT_VERSION: &str = "v1";

/// 单条最大重试次数（含首次）。
const MAX_ATTEMPTS: usize = 3;

/// 整理稿缓存（落 `douyin/refined/<aweme_id>.json`）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RefinedTranscript {
    pub aweme_id: String,
    /// 整理后的正文 + 小结（Markdown）。
    pub refined_text: String,
    /// 产出该整理稿的模型名。
    pub model: String,
    /// prompt 语义版本（[`PROMPT_VERSION`]）。
    pub prompt_version: String,
    /// prompt 文本 sm3 短哈希（16 hex）。prompt 改了哈希就变，可据此识别旧产物。
    pub prompt_hash: String,
    /// 整理时间（RFC3339）。
    pub refined_at: String,
}

/// LLM 连接配置（从环境变量装配）。
#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl LlmConfig {
    /// 从环境变量装配；缺 `LLM_BASE_URL` / `LLM_MODEL` 时明确报错（任务提交即失败）。
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
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model,
            api_key,
        })
    }
}

/// prompt 文本短哈希（sm3 前 8 字节 → 16 hex）。
pub fn prompt_hash() -> String {
    use sm3::{Digest, Sm3};
    let mut h = Sm3::new();
    h.update(REFINE_PROMPT.as_bytes());
    let out = h.finalize();
    out.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn refined_path(refined_dir: &Path, aweme_id: &str) -> PathBuf {
    refined_dir.join(format!("{aweme_id}.json"))
}

/// 选「已转写但未整理」的 aweme_id：扫 `transcript_dir/*.json`，排除 `refined_dir` 已有的。
/// 供 TextRefine 任务「全部待整理」模式与 pipeline 编排复用。返回已排序去重列表。
pub fn list_pending_refine(transcript_dir: &Path, refined_dir: &Path) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let rd = match std::fs::read_dir(transcript_dir) {
        Ok(r) => r,
        Err(_) => return Ok(ids),
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        // 跳过原子写残留的 .tmp（扩展名已被上面过滤，这里再防御 stem 带 .tmp）。
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem.is_empty() {
            continue;
        }
        if refined_path(refined_dir, stem).exists() {
            continue;
        }
        ids.push(stem.to_string());
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// 读单条整理稿缓存（供 knowledge 回填复用）。
pub fn read_refined(refined_dir: &Path, aweme_id: &str) -> Option<RefinedTranscript> {
    let raw = std::fs::read_to_string(refined_path(refined_dir, aweme_id)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content).with_context(|| format!("写临时文件 {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("替换 {}", path.display()))?;
    Ok(())
}

/// 整理单条 ASR 原文，落盘 `refined/<aweme_id>.json`。`ctx` 为该次调用的 trace 身份
/// （顶层任务 span 的 child），trace 关闭时为 no-op。
pub async fn refine_one(
    http: &reqwest::Client,
    cfg: &LlmConfig,
    refined_dir: &Path,
    aweme_id: &str,
    asr_text: &str,
) -> Result<RefinedTranscript> {
    refine_one_traced(http, cfg, refined_dir, aweme_id, asr_text, None).await
}

/// 带 trace 子 span 的整理（参考 asr-server `asr_decode` 子 span 做法）。
pub async fn refine_one_traced(
    http: &reqwest::Client,
    cfg: &LlmConfig,
    refined_dir: &Path,
    aweme_id: &str,
    asr_text: &str,
    parent: Option<&TraceContext>,
) -> Result<RefinedTranscript> {
    if asr_text.trim().is_empty() {
        bail!("ASR 原文为空，跳过整理");
    }
    std::fs::create_dir_all(refined_dir)
        .with_context(|| format!("建整理稿目录 {}", refined_dir.display()))?;

    // text_refine 子 span（两阶段）：anchor 先发，完成后 emit_end 覆盖。
    let scope = match (trace::enabled(), parent) {
        (true, Some(p)) => {
            let scope =
                trace::SpanScope::new(p.child(), "text_refine").with_summary(serde_json::json!({
                    "aweme_id": aweme_id,
                    "model": cfg.model,
                    "input_chars": asr_text.chars().count(),
                }));
            scope.emit_start();
            Some(scope)
        }
        _ => None,
    };

    let prompt = REFINE_PROMPT.replace("{TRANSCRIPT}", asr_text.trim());
    let result = chat_with_retry(http, cfg, &prompt).await;

    match result {
        Ok(refined_text) => {
            let refined = RefinedTranscript {
                aweme_id: aweme_id.to_string(),
                refined_text,
                model: cfg.model.clone(),
                prompt_version: PROMPT_VERSION.to_string(),
                prompt_hash: prompt_hash(),
                refined_at: chrono::Utc::now().to_rfc3339(),
            };
            atomic_write(
                &refined_path(refined_dir, aweme_id),
                &serde_json::to_string(&refined)?,
            )?;
            if let Some(s) = scope {
                s.emit_end(
                    Some(refined.refined_text.clone()),
                    SpanStatus::Ok,
                    Some(serde_json::json!({
                        "output_chars": refined.refined_text.chars().count(),
                    })),
                );
            }
            Ok(refined)
        }
        Err(e) => {
            if let Some(s) = scope {
                s.emit_end(
                    Some(format!("{e:#}")),
                    SpanStatus::Error(format!("{e:#}")),
                    Some(serde_json::json!({ "aweme_id": aweme_id })),
                );
            }
            Err(e)
        }
    }
}

/// 调一次 chat completions，失败按指数退避重试。
async fn chat_with_retry(http: &reqwest::Client, cfg: &LlmConfig, prompt: &str) -> Result<String> {
    let mut last_err = None;
    for attempt in 1..=MAX_ATTEMPTS {
        match chat_once(http, cfg, prompt).await {
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

/// 单次 chat completions 调用，返回 assistant 消息文本。
async fn chat_once(http: &reqwest::Client, cfg: &LlmConfig, prompt: &str) -> Result<String> {
    let url = format!("{}/chat/completions", cfg.base_url);
    let body = serde_json::json!({
        "model": cfg.model,
        "messages": [{ "role": "user", "content": prompt }],
        "temperature": 0.2,
        "stream": false,
    });
    let mut req = http.post(&url).json(&body);
    if let Some(key) = &cfg.api_key {
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
    fn prompt_has_placeholder() {
        assert!(REFINE_PROMPT.contains("{TRANSCRIPT}"));
    }

    #[test]
    fn prompt_hash_stable_and_16_hex() {
        let h = prompt_hash();
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h, prompt_hash());
    }

    #[test]
    fn refined_roundtrip() {
        let dir = std::env::temp_dir().join(format!("refine-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = RefinedTranscript {
            aweme_id: "7a".into(),
            refined_text: "整理后的正文。\n\n## 小结\n讲了 X。".into(),
            model: "qwen".into(),
            prompt_version: PROMPT_VERSION.into(),
            prompt_hash: prompt_hash(),
            refined_at: "2026-06-10T00:00:00Z".into(),
        };
        atomic_write(
            &refined_path(&dir, "7a"),
            &serde_json::to_string(&r).unwrap(),
        )
        .unwrap();
        let back = read_refined(&dir, "7a").unwrap();
        assert_eq!(back.refined_text, r.refined_text);
        assert_eq!(back.prompt_version, "v1");
        assert!(read_refined(&dir, "nope").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_pending_excludes_already_refined() {
        let base = std::env::temp_dir().join(format!("refine-pending-{}", std::process::id()));
        let tr = base.join("transcripts");
        let rf = base.join("refined");
        std::fs::create_dir_all(&tr).unwrap();
        std::fs::create_dir_all(&rf).unwrap();
        // 三条转写：7a/7b/7c；其中 7b 已有整理稿。
        for id in ["7a", "7b", "7c"] {
            std::fs::write(tr.join(format!("{id}.json")), "{}").unwrap();
        }
        std::fs::write(rf.join("7b.json"), "{}").unwrap();
        let pending = list_pending_refine(&tr, &rf).unwrap();
        assert_eq!(pending, vec!["7a".to_string(), "7c".to_string()]);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn list_pending_missing_dir_is_empty() {
        let nope = std::env::temp_dir().join("refine-nope-xyz-123");
        assert!(list_pending_refine(&nope, &nope).unwrap().is_empty());
    }

    #[test]
    fn chat_response_parses() {
        let raw = r#"{"choices":[{"message":{"role":"assistant","content":"hello"}}]}"#;
        let p: ChatResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(p.choices[0].message.content, "hello");
    }
}
