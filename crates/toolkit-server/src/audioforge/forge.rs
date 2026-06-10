//! Phase 3：`audio_forge` TaskKind —— 文本句子清单 → 逐句 TTS → 打包学习包草稿。
//!
//! 形态（与 `douyin_text_refine` 同源：进程内逐条调上游、单条失败不拖垮整批）：
//! 1. 解析输入：显式 `sentences[]`，或 `from_refined`（从抖音整理稿抽句，见下）。
//! 2. 逐句调上游 TTS（`TTS_BASE_URL/tts`，复用 Phase 1 配置约定）→ 落
//!    `<workspace>/audioforge/<package_id>/NNN.wav`。单句失败重试，最终失败进 `failures[]`。
//! 3. 解析 WAV 头得到时长 → 写 manifest.json（包元信息 + 句子数组）。
//! 4. trace：`audio_forge_batch` 顶层 span + 逐句 `tts_one` 子 span（参考 refine 做法）。
//!
//! 产物即「学习包草稿」，由 `/api/web/audio/forge/{package_id}/...` 暴露给 english 拉取。

use crate::audioforge::manifest::{
    audio_file_name, ForgePaths, Manifest, ManifestFailure, ManifestSentence, MANIFEST_VERSION,
};
use crate::audioforge::tts::{tts_base_url, TtsClient};
use crate::audioforge::wav::wav_duration_secs;
use crate::douyin_mod::paths::DouyinPaths;
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use custom_utils::trace::{self, SpanScope, SpanStatus, TraceContext};
use serde::{Deserialize, Serialize};
use serde_json::json;
use toolkit_core::new_task_id;
use toolkit_tasks::{TaskCtx, TaskKind};

/// 单句输入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentenceInput {
    pub text: String,
    #[serde(default)]
    pub translation: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// 逐句覆盖音色（缺省用包级 voice_id）。
    #[serde(default)]
    pub voice_id: Option<String>,
}

/// 从抖音整理稿抽句的快捷来源。
///
/// **当前实现（待迭代）**：把指定 aweme_id 的整理稿全文按句切分（标点切分），
/// 不做「英语片段精选」——英语片段抽取逻辑后续迭代，见 runbook 说明。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FromRefined {
    /// 抖音号（仅回显）。
    #[serde(default)]
    pub unique_id: Option<String>,
    /// 要抽句的整理稿 aweme_id 列表。
    pub aweme_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeInput {
    /// 包名（→ english packages.title）。
    pub package_name: String,
    /// 专题，可空。
    #[serde(default)]
    pub topic: Option<String>,
    /// 语言标签，默认 `en`。
    #[serde(default = "default_language")]
    pub language: String,
    /// 包级统一音色 id（必填）。
    pub voice_id: String,
    /// 包级 TTS 参数（语速 / instruct 等），逐句透传给上游；可空。
    #[serde(default)]
    pub tts_params: serde_json::Value,
    /// 显式句子清单。
    #[serde(default)]
    pub sentences: Vec<SentenceInput>,
    /// 或从抖音整理稿抽句（与 sentences 二选一 / 可叠加）。
    #[serde(default)]
    pub from_refined: Option<FromRefined>,
}

fn default_language() -> String {
    "en".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ForgeOutput {
    pub package_id: String,
    pub package_name: String,
    pub total: usize,
    pub generated: usize,
    pub failed: usize,
    pub voice_id: String,
    pub source: String,
    /// manifest 在 workspace 内的相对路径（如 `audioforge/<id>/manifest.json`）。
    pub manifest_path: String,
    /// 可访问的 manifest URL 路径（供 english 拼 base 后拉取）。
    pub manifest_url: String,
    pub failures: Vec<ManifestFailure>,
}

pub struct AudioForge;

#[async_trait]
impl TaskKind for AudioForge {
    type Input = ForgeInput;
    type Output = ForgeOutput;
    const KIND: &'static str = "audio_forge";

    async fn run(input: ForgeInput, ctx: TaskCtx) -> Result<ForgeOutput> {
        // 提交即校验上游 TTS 配置：未配置 TTS_BASE_URL 明确报错（不进入逐句循环）。
        let base = tts_base_url()
            .context("未配置 TTS_BASE_URL（上游 CosyVoice2，如 http://127.0.0.1:8095）")?;
        if input.voice_id.trim().is_empty() {
            bail!("voice_id 不能为空");
        }

        // 汇集句子来源：显式 sentences + from_refined 抽句。
        let (sentences, source) = collect_sentences(&input, &ctx)?;
        if sentences.is_empty() {
            bail!("无可生成的句子（sentences 为空且 from_refined 未抽到任何句）");
        }

        let paths = ForgePaths::new(&ctx.data_dir);
        let package_id = new_task_id();
        let pkg_dir = paths.package_dir(&package_id);
        std::fs::create_dir_all(&pkg_dir)
            .with_context(|| format!("建包目录 {}", pkg_dir.display()))?;

        let total = sentences.len();
        ctx.report_progress(json!({
            "stage": "forge",
            "package_id": package_id,
            "total": total,
            "done": 0,
            "generated": 0,
            "failed": 0,
        }))?;

        // 顶层 span（两阶段）：每句 TTS 调用作其子 span。
        let batch_ctx = trace::enabled().then(TraceContext::root);
        let batch_scope = batch_ctx.as_ref().map(|c| {
            let scope = SpanScope::new(c.clone(), "audio_forge_batch")
                .with_flow_name("audio_forge")
                .with_summary(json!({
                    "package_id": package_id,
                    "total": total,
                    "voice_id": input.voice_id,
                }));
            scope.emit_start();
            scope
        });

        let tts = TtsClient::new(base)?;
        let mut out_sentences: Vec<ManifestSentence> = Vec::new();
        let mut failures: Vec<ManifestFailure> = Vec::new();

        for (i, s) in sentences.iter().enumerate() {
            let index = i + 1;
            let voice = s.voice_id.clone().unwrap_or_else(|| input.voice_id.clone());
            let file_name = audio_file_name(index);
            let wav_path = pkg_dir.join(&file_name);

            match tts
                .synthesize_traced(&s.text, &voice, &input.tts_params, batch_ctx.as_ref())
                .await
            {
                Ok(bytes) => {
                    if let Err(e) = std::fs::write(&wav_path, &bytes) {
                        failures.push(ManifestFailure {
                            index,
                            text: s.text.clone(),
                            error: format!("写音频文件失败: {e}"),
                        });
                    } else {
                        out_sentences.push(ManifestSentence {
                            index,
                            text: s.text.clone(),
                            translation: s.translation.clone(),
                            note: s.note.clone(),
                            audio_file: file_name,
                            duration: wav_duration_secs(&bytes),
                            voice_id: voice,
                            tts_params: input.tts_params.clone(),
                            generated_at: chrono::Utc::now().to_rfc3339(),
                        });
                    }
                }
                Err(e) => failures.push(ManifestFailure {
                    index,
                    text: s.text.clone(),
                    error: format!("{e:#}"),
                }),
            }

            ctx.report_progress(json!({
                "stage": "forge",
                "package_id": package_id,
                "total": total,
                "done": index,
                "generated": out_sentences.len(),
                "failed": failures.len(),
            }))?;
        }

        let manifest = Manifest {
            manifest_version: MANIFEST_VERSION,
            package_id: package_id.clone(),
            package_name: input.package_name.clone(),
            topic: input.topic.clone(),
            language: input.language.clone(),
            voice_id: input.voice_id.clone(),
            source: source.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            total,
            sentences: out_sentences,
            failures: failures.clone(),
        };
        let manifest_json = serde_json::to_string_pretty(&manifest).context("序列化 manifest")?;
        std::fs::write(paths.manifest_path(&package_id), &manifest_json)
            .context("写 manifest.json")?;

        if let Some(scope) = batch_scope {
            let status = if failures.is_empty() {
                SpanStatus::Ok
            } else {
                SpanStatus::Error(format!("{} 句失败", failures.len()))
            };
            scope.emit_end(
                None,
                status,
                Some(json!({
                    "total": total,
                    "generated": manifest.sentences.len(),
                    "failed": failures.len(),
                })),
            );
        }

        Ok(ForgeOutput {
            package_id: package_id.clone(),
            package_name: input.package_name,
            total,
            generated: manifest.sentences.len(),
            failed: failures.len(),
            voice_id: input.voice_id,
            source,
            manifest_path: format!("audioforge/{package_id}/manifest.json"),
            manifest_url: format!("/api/web/audio/forge/{package_id}/manifest.json"),
            failures,
        })
    }
}

/// 汇集句子来源，返回 (句子列表, source 标记)。
fn collect_sentences(input: &ForgeInput, ctx: &TaskCtx) -> Result<(Vec<SentenceInput>, String)> {
    let mut out: Vec<SentenceInput> = input.sentences.clone();
    let mut from_refined = false;

    if let Some(fr) = &input.from_refined {
        from_refined = true;
        let dpaths = DouyinPaths::new(&ctx.data_dir);
        for id in &fr.aweme_ids {
            let refined = douyin::refine::read_refined(&dpaths.refined_dir, id)
                .with_context(|| format!("整理稿不存在或读取失败: {id}"))?;
            for text in split_sentences(&refined.refined_text) {
                out.push(SentenceInput {
                    text,
                    translation: None,
                    note: None,
                    voice_id: None,
                });
            }
        }
    }

    let source = match (from_refined, !input.sentences.is_empty()) {
        (true, true) => "mixed",
        (true, false) => "from_refined",
        _ => "manual",
    };
    Ok((out, source.to_string()))
}

/// 把一段文本按句切分（句末标点 + 换行）。**简化实现，待迭代**为「英语片段精选」。
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        // markdown 标题 # 与列表符跳过（避免把「## 小结」当成句子内容）。
        buf.push(ch);
        if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | '\n') {
            let s = clean_sentence(&buf);
            if !s.is_empty() {
                sentences.push(s);
            }
            buf.clear();
        }
    }
    let tail = clean_sentence(&buf);
    if !tail.is_empty() {
        sentences.push(tail);
    }
    sentences
}

/// 清洗单句：去首尾空白、剥离 markdown 标题 / 列表前缀。
fn clean_sentence(raw: &str) -> String {
    let s = raw.trim();
    let s = s.trim_start_matches(['#', '-', '*', '>', ' ']).trim();
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_basic_sentences() {
        let r = split_sentences("Hello world. How are you? Fine!");
        assert_eq!(r, vec!["Hello world.", "How are you?", "Fine!"]);
    }

    #[test]
    fn split_strips_markdown_and_blank() {
        let r = split_sentences("## 小结\n- One thing.\n\nTwo things.");
        assert_eq!(r, vec!["小结", "One thing.", "Two things."]);
    }

    #[test]
    fn collect_manual_only() {
        // 仅显式句子时 source=manual（不触达文件系统）。
        let input = ForgeInput {
            package_name: "p".into(),
            topic: None,
            language: "en".into(),
            voice_id: "v".into(),
            tts_params: serde_json::Value::Null,
            sentences: vec![SentenceInput {
                text: "One.".into(),
                translation: None,
                note: None,
                voice_id: None,
            }],
            from_refined: None,
        };
        let ctx = TaskCtx {
            task_id: "t".into(),
            pool: toolkit_core::open_pool(std::path::Path::new(":memory:")).unwrap(),
            data_dir: std::env::temp_dir(),
        };
        let (s, src) = collect_sentences(&input, &ctx).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(src, "manual");
    }
}
