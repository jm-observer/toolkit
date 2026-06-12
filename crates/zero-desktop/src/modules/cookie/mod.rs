use anyhow::Result;
use std::sync::Arc;

/// Cookie 模块状态（阶段 1 为空占位）。
#[derive(Default)]
pub struct CookieState {}

/// 初始化 Cookie 模块（阶段 1 为空实现）。
pub fn setup(_app: &tauri::AppHandle, _state: Arc<CookieState>) -> Result<()> {
    Ok(())
}

#[tauri::command]
pub fn cookie_ping() -> &'static str {
    "ok"
}
