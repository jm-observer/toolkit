//! workspace 根：通过 `custom_utils::util_args::workspace` 统一解析。
//!
//! 优先级：CLI `--workspace` > 环境变量 `TOOLKIT_DESKTOP_WORKSPACE`
//! > `$HOME/.config/toolkit-desktop`（Windows = `C:\Users\<u>\.config\toolkit-desktop`）。
//!
//! 布局：
//! ```text
//! <workspace>/config.json     — Settings 序列化
//! <workspace>/state.db        — SQLite，记录上传历史
//! <workspace>/logs/           — prod feature 下 custom-utils logger 落盘位置
//! ```

use anyhow::{Context, Result};
use std::path::PathBuf;

pub const APP: &str = "toolkit-desktop";

pub fn resolve(arg: &Option<String>) -> Result<PathBuf> {
    let env_arg = match arg {
        Some(_) => arg.clone(),
        None => std::env::var("TOOLKIT_DESKTOP_WORKSPACE").ok(),
    };
    let ws = custom_utils::args::workspace(&env_arg, APP)?;
    std::fs::create_dir_all(&ws).with_context(|| format!("create workspace {}", ws.display()))?;
    Ok(ws)
}

pub fn config_path(ws: &std::path::Path) -> PathBuf {
    ws.join("config.json")
}

pub fn db_path(ws: &std::path::Path) -> PathBuf {
    ws.join("state.db")
}
