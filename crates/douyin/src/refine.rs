//! Phase 2：LLM 整理文本（TextRefine）。
//!
//! 输入 ASR 原文 → 调大模型（OpenAI 兼容 chat completions）做纠错 / 去口语水词 / 分段 / 小结 →
//! 产出「整理稿」。整理稿落 `douyin/refined/<aweme_id>.json`，与 `douyin/transcripts/<aweme_id>.json`
//! （ASR 原文）并列；`knowledge::run_publish_knowledge` 读取它并写进 knowledge md 的「整理稿」栏。
//!
//! ## 连接配置 / 提示词
//! 本模块**不再自行装配连接配置或读取提示词**——大模型连接（[`toolkit_llm::LlmClient`]）与提示词
//! 模板由调用方（toolkit-server 的 `llm` 层，DB 可配 + env 兜底）解析后传入。本文件仅保留
//! [`REFINE_PROMPT`] / [`PROMPT_VERSION`] 作为该提示词的**编译期内置默认**（被 toolkit-server 的
//! 可配提示词目录登记为 `douyin_refine`）。
//!
//! ## prompt 溯源
//! 每条整理稿记录 `prompt_version`（人工语义版本）与 `prompt_hash`（实际生效提示词模板的 sm3
//! 短哈希）。提示词被 DB 覆盖后哈希随之变化，可识别哪些条目是旧提示词产物、按需重跑对比。

use anyhow::{bail, Context, Result};
use custom_utils::trace::{self, SpanStatus, TraceContext};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use toolkit_llm::LlmClient;

/// 内置整理 prompt（随 crate 编译，作为 `douyin_refine` 提示词的默认值）。`{TRANSCRIPT}`
/// 占位符在调用时替换为 ASR 原文。
pub const REFINE_PROMPT: &str = include_str!("refine_prompt.md");

/// prompt 人工语义版本。迭代内置 prompt 文案时同步 bump。
pub const PROMPT_VERSION: &str = "v1";

/// 整理稿缓存（落 `douyin/refined/<aweme_id>.json`）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RefinedTranscript {
    pub aweme_id: String,
    /// 整理后的正文 + 小结（Markdown）。
    pub refined_text: String,
    /// 产出该整理稿的模型名。
    pub model: String,
    /// prompt 语义版本（生效提示词的版本）。
    pub prompt_version: String,
    /// 生效 prompt 模板文本的 sm3 短哈希（16 hex）。
    pub prompt_hash: String,
    /// 整理时间（RFC3339）。
    pub refined_at: String,
}

/// 内置默认 prompt 的短哈希（供测试 / 默认产物元信息）。生效提示词的哈希在 [`refine_one_traced`]
/// 内按传入模板实时计算。
pub fn prompt_hash() -> String {
    toolkit_llm::prompt_hash(REFINE_PROMPT)
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

/// 整理单条 ASR 原文，落盘 `refined/<aweme_id>.json`。`client` 为解析好的大模型客户端，
/// `prompt_template`/`prompt_version` 为生效提示词（含 `{TRANSCRIPT}` 占位符）及其版本。
pub async fn refine_one(
    client: &LlmClient,
    prompt_template: &str,
    prompt_version: &str,
    refined_dir: &Path,
    aweme_id: &str,
    asr_text: &str,
) -> Result<RefinedTranscript> {
    refine_one_traced(
        client,
        prompt_template,
        prompt_version,
        refined_dir,
        aweme_id,
        asr_text,
        None,
    )
    .await
}

/// 带 trace 子 span 的整理。`parent` 为顶层任务 span 的身份（trace 关闭时为 None → no-op）。
#[allow(clippy::too_many_arguments)]
pub async fn refine_one_traced(
    client: &LlmClient,
    prompt_template: &str,
    prompt_version: &str,
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
                    "model": client.model(),
                    "input_chars": asr_text.chars().count(),
                }));
            scope.emit_start();
            Some(scope)
        }
        _ => None,
    };

    let hash = toolkit_llm::prompt_hash(prompt_template);
    let prompt = prompt_template.replace("{TRANSCRIPT}", asr_text.trim());
    let result = client.complete(&prompt).await;

    match result {
        Ok(refined_text) => {
            let refined = RefinedTranscript {
                aweme_id: aweme_id.to_string(),
                refined_text,
                model: client.model().to_string(),
                prompt_version: prompt_version.to_string(),
                prompt_hash: hash,
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
}
