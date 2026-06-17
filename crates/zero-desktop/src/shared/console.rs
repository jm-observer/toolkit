//! 通用「在系统浏览器打开 URL」命令——供 G10 部署页等模块复用。
//!
//! 复用已装的 `tauri-plugin-shell`（capability `shell:default` 已授权），不引入 opener 插件，
//! 与 `speech_open_in_folder` 同一套做法。

use tauri_plugin_shell::ShellExt;

/// 在系统默认浏览器打开给定 URL。前端用 `invoke('open_url', { url })` 调用。
#[tauri::command]
pub async fn open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("URL 为空".into());
    }
    #[allow(deprecated)]
    app.shell()
        .open(url, None)
        .map_err(|e| format!("打开 URL 失败: {e}"))
}
