//! AudioForge「学习包草稿」的 manifest 数据模型 + 路径解析。
//!
//! 一个学习包 = `<workspace>/audioforge/<package_id>/` 目录：
//!   - `NNN.wav`（逐句音频，001 起，三位补零）
//!   - `manifest.json`（本文件的 [`Manifest`]：包元信息 + 句子数组）
//!
//! manifest 即 english `package.import` 的契约载体：english 拉取它 + 音频，落库消费。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// manifest schema 版本：契约变更时 bump，english 侧据此判断兼容性。
pub const MANIFEST_VERSION: u32 = 1;

/// 学习包 manifest（落 `<package_id>/manifest.json`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub manifest_version: u32,
    /// 包唯一 id（= 目录名，也是下载 URL 段）。
    pub package_id: String,
    /// 包名（english 侧 packages.title）。
    pub package_name: String,
    /// 专题（如「数字」「问路」），可空。
    #[serde(default)]
    pub topic: Option<String>,
    /// 语言标签（如 `en`），默认 `en`。
    pub language: String,
    /// 统一音色 id（逐句若未单独指定则用它，仅回显）。
    pub voice_id: String,
    /// 产出该包的来源标记：`manual` / `from_refined` 等。
    pub source: String,
    /// 生成时间（RFC3339）。
    pub created_at: String,
    /// 句子总数（= sentences.len()，含失败占位？否：失败句不入 sentences，进 failures）。
    pub total: usize,
    /// 成功生成音频的句子。
    pub sentences: Vec<ManifestSentence>,
    /// 生成失败的句子（重试后仍失败），不拖垮整批。
    #[serde(default)]
    pub failures: Vec<ManifestFailure>,
}

/// manifest 里的单句记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestSentence {
    /// 句序号（1 起，与文件名 NNN 对应）。
    pub index: usize,
    /// 英文文本。
    pub text: String,
    /// 可选译文。
    #[serde(default)]
    pub translation: Option<String>,
    /// 可选注释。
    #[serde(default)]
    pub note: Option<String>,
    /// 音频文件名（相对 package 目录，如 `001.wav`）。
    pub audio_file: String,
    /// 音频时长（秒，解析 WAV 头得到；解析失败为 null）。
    #[serde(default)]
    pub duration: Option<f64>,
    /// 该句实际使用的音色 id。
    pub voice_id: String,
    /// 该句使用的 TTS 参数（语速 / instruct 等，原样回显）。
    #[serde(default)]
    pub tts_params: serde_json::Value,
    /// 该句音频生成时间（RFC3339）。
    pub generated_at: String,
}

/// 失败句记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFailure {
    pub index: usize,
    pub text: String,
    pub error: String,
}

/// AudioForge 在 workspace 下的路径解析。
#[derive(Debug, Clone)]
pub struct ForgePaths {
    /// `<workspace>/audioforge/`。
    pub root: PathBuf,
}

impl ForgePaths {
    pub fn new(workspace: &Path) -> Self {
        Self {
            root: workspace.join("audioforge"),
        }
    }

    /// 某个包的目录 `<root>/<package_id>/`。
    pub fn package_dir(&self, package_id: &str) -> PathBuf {
        self.root.join(package_id)
    }

    /// 某个包的 manifest 路径。
    pub fn manifest_path(&self, package_id: &str) -> PathBuf {
        self.package_dir(package_id).join("manifest.json")
    }
}

/// 句序号 → 文件名（`001.wav`）。
pub fn audio_file_name(index: usize) -> String {
    format!("{index:03}.wav")
}
