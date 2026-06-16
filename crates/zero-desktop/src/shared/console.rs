//! 打开 toolkit-server web 控制台——读 app.json 里的 `console_url`，用系统默认浏览器打开。
//!
//! 复用已装的 `tauri-plugin-shell`（capability `shell:default` 已授权），不引入 opener 插件，
//! 与 `speech_open_in_folder` 同一套做法。

use crate::app_state::AppState;
use crate::shared::settings;
use tauri_plugin_shell::ShellExt;

/// 在系统浏览器打开 toolkit-server 控制台（地址取 app 设置里的 `console_url`）。
#[tauri::command]
pub async fn open_toolkit_console(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let s = settings::load_app_settings(&state.workspace);
    let url = s.console_url.trim().to_string();
    if url.is_empty() {
        return Err("控制台地址未配置，请到设置页填写".into());
    }
    #[allow(deprecated)]
    app.shell()
        .open(url, None)
        .map_err(|e| format!("打开控制台失败: {e}"))
}
