use anyhow::Result;
use std::sync::Arc;

/// Speech 模块状态（阶段 1 为空占位）。
#[derive(Default)]
pub struct SpeechState {}

/// 初始化 Speech 模块（阶段 1 为空实现）。
pub fn setup(_app: &tauri::AppHandle, _state: Arc<SpeechState>) -> Result<()> {
    Ok(())
}

#[tauri::command]
pub fn speech_ping() -> &'static str {
    "ok"
}
