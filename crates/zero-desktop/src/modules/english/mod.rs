use anyhow::Result;
use std::sync::Arc;
use tauri::State;

use crate::{app_state::AppState, shared::settings::load_app_settings};

/// English 模块状态（前端通过 plugin-store / plugin-fs 管理自身 KV 和音频缓存）。
#[derive(Default)]
pub struct EnglishState {}

/// 初始化 English 模块：确保 audio-cache 目录已存在（workspace 初始化已创建，此处为文档化）。
pub fn setup(_app: &tauri::AppHandle, _state: Arc<EnglishState>) -> Result<()> {
    Ok(())
}

/// 健康探针。
#[tauri::command]
pub fn english_ping() -> &'static str {
    "ok"
}

/// 返回 app.json 中配置的 g10_base（用于 ApiService 的 apiBase）。
/// 若未配置返回空字符串，前端应抛错引导用户到设置页配置。
#[tauri::command]
pub fn english_get_g10_base(state: State<'_, AppState>) -> String {
    load_app_settings(&state.workspace).g10_base
}

/// 返回 english 音频缓存目录的绝对路径（用于 FileCacheManager 的根路径）。
#[tauri::command]
pub fn english_get_audio_cache_dir(state: State<'_, AppState>) -> String {
    crate::shared::workspace::english_audio_cache_dir(&state.workspace)
        .to_string_lossy()
        .into_owned()
}
