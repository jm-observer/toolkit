//! 桌面端运行期配置，落盘在 `<workspace>/config.json`。
//!
//! CLI / 环境变量优先级：`--server` > `TOOLKIT_DESKTOP_SERVER` > 配置文件。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Server 指向 toolkit-server，cookie 端点固定 `/api/browser/cookie`。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// 例：`https://g10.example.com` 或 `http://127.0.0.1:8788`（不含路径）。
    #[serde(default)]
    pub server_base: String,
    /// 可选 Bearer token（若 server 启用了鉴权）。MVP 不强制。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    /// 上次成功上传时间（UI 显示用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_uploaded_at: Option<String>,
}

impl Settings {
    pub fn is_configured(&self) -> bool {
        !self.server_base.trim().is_empty()
    }

    pub fn cookie_endpoint(&self) -> Option<String> {
        if !self.is_configured() {
            return None;
        }
        let base = self.server_base.trim_end_matches('/');
        Some(format!("{base}/api/browser/cookie"))
    }
}

pub fn load(path: &Path) -> Settings {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Settings::default(),
        Err(e) => {
            log::warn!("settings read {} failed: {e}", path.display());
            return Settings::default();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("settings parse {} failed: {e}", path.display());
            Settings::default()
        }
    }
}

pub fn save(path: &Path, s: &Settings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(s)?;
    std::fs::write(path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
