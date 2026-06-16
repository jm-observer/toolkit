//! 全局应用配置，落盘在 `{workspace}/app.json`。
//!
//! 当前存储 G10 server base / token，供 cookie 模块和 english 模块共享。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// G10 server base，例如 `http://192.168.1.100:8788`（不含路径）。
    #[serde(default = "default_g10_base")]
    pub g10_base: String,
    /// 可选 Bearer token（若 G10 server 启用了鉴权）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub g10_token: Option<String>,
    /// toolkit-server web 控制台地址——直连 toolkit-server（不走公网反代，反代只转
    /// `/api/web/*`，控制台静态页在 `/`、`/hub` 等路径上）。例如 `http://192.168.0.68:8788`。
    #[serde(default = "default_console_url")]
    pub console_url: String,
}

fn default_g10_base() -> String {
    "https://www.for-memory.cloud:28080".to_string()
}

fn default_console_url() -> String {
    "http://192.168.0.68:8788".to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            g10_base: default_g10_base(),
            g10_token: None,
            console_url: default_console_url(),
        }
    }
}

impl AppSettings {
    pub fn is_configured(&self) -> bool {
        !self.g10_base.trim().is_empty()
    }

    pub fn cookie_endpoint(&self) -> Option<String> {
        if !self.is_configured() {
            return None;
        }
        let base = self.g10_base.trim_end_matches('/');
        Some(format!("{base}/api/browser/cookie"))
    }

    /// 音频清洗代理端点 `{g10_base}/api/web/audio/clean`（与 TTS/cookie 同源，复用全局
    /// g10_base）。未配置 g10_base 时返回 None。
    pub fn clean_endpoint(&self) -> Option<String> {
        if !self.is_configured() {
            return None;
        }
        let base = self.g10_base.trim_end_matches('/');
        Some(format!("{base}/api/web/audio/clean"))
    }

    /// TTS 代理端点 `{g10_base}/api/web/audio/tts`（toolkit-server 代理 → CosyVoice2）。
    pub fn tts_endpoint(&self) -> Option<String> {
        if !self.is_configured() {
            return None;
        }
        let base = self.g10_base.trim_end_matches('/');
        Some(format!("{base}/api/web/audio/tts"))
    }

    /// 音色库端点 `{g10_base}/api/web/audio/voices`。
    pub fn voices_endpoint(&self) -> Option<String> {
        if !self.is_configured() {
            return None;
        }
        let base = self.g10_base.trim_end_matches('/');
        Some(format!("{base}/api/web/audio/voices"))
    }

    /// 公共大模型层端点 `{g10_base}/api/web/llm{path}`（`path` 以 `/` 开头，如 `/config`）。
    /// 与 TTS/cookie 同源，复用全局 g10_base。未配置 g10_base 时返回 None。
    pub fn llm_endpoint(&self, path: &str) -> Option<String> {
        if !self.is_configured() {
            return None;
        }
        let base = self.g10_base.trim_end_matches('/');
        Some(format!("{base}/api/web/llm{path}"))
    }

    /// 句子整体替换端点 `{g10_base}/api/sentence/replace-audio`（english 后端，非 toolkit-server；
    /// 走 `/api` 前缀以匹配反代「`/api/*`→english、`/api/web/*`→toolkit-server」的路由规则）。
    pub fn replace_sentence_audio_endpoint(&self) -> Option<String> {
        if !self.is_configured() {
            return None;
        }
        let base = self.g10_base.trim_end_matches('/');
        Some(format!("{base}/api/sentence/replace-audio"))
    }
}

pub fn app_settings_path(workspace: &Path) -> PathBuf {
    workspace.join("app.json")
}

pub fn load_app_settings(workspace: &Path) -> AppSettings {
    let path = app_settings_path(workspace);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return AppSettings::default(),
        Err(e) => {
            log::warn!("app.json read {} failed: {e}", path.display());
            return AppSettings::default();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("app.json parse {} failed: {e}", path.display());
            AppSettings::default()
        }
    }
}

pub fn save_app_settings(workspace: &Path, s: &AppSettings) -> Result<()> {
    let path = app_settings_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(s)?;
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
