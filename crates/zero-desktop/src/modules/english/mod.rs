use anyhow::Result;
use std::sync::Arc;

/// English 模块状态（阶段 1 为空占位）。
#[derive(Default)]
pub struct EnglishState {}

/// 初始化 English 模块（阶段 1 为空实现）。
pub fn setup(_app: &tauri::AppHandle, _state: Arc<EnglishState>) -> Result<()> {
    Ok(())
}

#[tauri::command]
pub fn english_ping() -> &'static str {
    "ok"
}
