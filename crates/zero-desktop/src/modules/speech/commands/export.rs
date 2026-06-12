use tauri_plugin_clipboard_manager::ClipboardExt;
use tracing::info;

#[tauri::command]
pub fn speech_copy_text_to_clipboard(app: tauri::AppHandle, text: String) -> Result<(), String> {
    info!(target: "speech", "[speech_copy_text_to_clipboard] text_len={}", text.len());
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}
