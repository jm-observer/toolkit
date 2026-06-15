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
}

fn default_g10_base() -> String {
    "https://www.for-memory.cloud:28080".to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            g10_base: default_g10_base(),
            g10_token: None,
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
