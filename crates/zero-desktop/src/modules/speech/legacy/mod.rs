//! Legacy / disabled speech features.
//!
//! Per §4.2.1 of the unified-desktop-shell design:
//!
//! - **Window size/always-on-top/compact-mode** (`resize_window`,
//!   `set_always_on_top`, `set_min_size`, simple-mode toggle): originally in
//!   streaming-speech `src-tauri/src/App.tsx` lines 543-566
//!   (`applyWindowMode` effect) and `src/commands/*.rs`. Moved here because
//!   the shell main window must not be resized or pinned by a module.
//!
//! - **Window attention flash** (`requestUserAttention(Critical)`:
//!   streaming-speech `src/src/App.tsx` lines 115-118 (`getCurrentWindow()
//!   .requestUserAttention(UserAttentionType.Critical)`). Moved here because
//!   `window.hide()` removes the taskbar button, making `FlashWindowEx` /
//!   `request_user_attention` a no-op when minimised. The tray bounce
//!   (`commands/notify.rs::bounce_tray_twice`) is the replacement and stays
//!   enabled.
//!
//! - **Auto-recording** (`AUTO_RECORDING_STORAGE_KEY` + `autoStartTriggered`
//!   logic, streaming-speech `src/src/App.tsx` lines 331-353): disabled in
//!   the shell because auto-start of recording is a strong-interrupt behaviour
//!   that should be a per-module opt-in setting, not an implicit default.
//!
//! - **Tray "quick start recording" item**: streaming-speech added a menu item
//!   that starts recording from the tray. Per §4.2.1 the tray menu in the
//!   shell's first version only has "Show window" and "Quit". The item stays
//!   here until stage 5 (floating mini-window evaluation).
//!
//! All code below compiles but is gated with `#[allow(dead_code)]`; it will
//! be re-enabled in stage 5.

#![allow(dead_code)]

use tauri::Manager;
use tracing::warn;

/// Apply simple/detailed window mode (disabled: shell window is user-controlled).
///
/// Source: streaming-speech `src/src/App.tsx` `applyWindowMode` effect +
/// `core:window:allow-set-size` / `allow-set-always-on-top` capability entries.
pub async fn apply_window_mode_legacy(app: &tauri::AppHandle, simple: bool) {
    let Some(window) = app.get_webview_window("main") else {
        warn!(target: "speech", "[legacy] main window not found for window mode");
        return;
    };
    if simple {
        let _ = window.set_min_size(None::<tauri::LogicalSize<f64>>);
        let _ = window.set_always_on_top(true);
        let _ = window.set_size(tauri::LogicalSize::new(560.0_f64, 280.0_f64));
    } else {
        let _ = window.set_always_on_top(false);
        let _ = window.set_min_size(Some(tauri::LogicalSize::new(900.0_f64, 600.0_f64)));
        let _ = window.set_size(tauri::LogicalSize::new(1280.0_f64, 820.0_f64));
    }
}

/// Request window attention flash (disabled: window may be hidden to tray).
///
/// Source: streaming-speech `src/src/App.tsx` line 116:
/// `getCurrentWindow().requestUserAttention(UserAttentionType.Critical)`.
/// The replacement is `commands::notify::bounce_tray_twice` which animates
/// the tray icon instead.
pub async fn request_window_attention_legacy(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        warn!(target: "speech", "[legacy] main window not found for attention request");
        return;
    };
    let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
}

/// Tray menu item: "Quick start recording" (disabled: tray only has Show + Quit).
///
/// Source: streaming-speech `src-tauri/src/lib.rs` tray builder; the menu item
/// would invoke `speech_start_recording` directly from the tray. Disabled per
/// §4.2.1 until stage 5 evaluates the floating mini-window concept.
pub fn build_quick_record_tray_item_legacy() {
    // Placeholder — no-op until stage 5.
}
