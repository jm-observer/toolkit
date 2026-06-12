//! System-tray "bounce" + short audio notification.
//!
//! When a segment finishes both LLM polish and translation, we briefly toggle
//! the system-tray icon visibility twice to grab attention, optionally
//! accompanied by two short system "beeps".
//!
//! MessageBeep (Windows-only) is kept as it is a system sound, not a window
//! attention call. `request_user_attention` is in legacy/ per §4.2.1.

use std::time::Duration;

use tracing::{info, warn};

const BOUNCE_CYCLES: usize = 2;
const BOUNCE_STEP: Duration = Duration::from_millis(320);

pub fn bounce_tray_twice(app: &tauri::AppHandle, play_beep: bool) {
    info!(
        target: "speech",
        "[notify] bounce_tray_twice invoked (beep={} cycles={} step={:?})",
        play_beep, BOUNCE_CYCLES, BOUNCE_STEP
    );
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(tray) = app.tray_by_id("main") else {
            warn!(target: "speech", "[notify] tray 'main' not found");
            return;
        };
        let Some(icon) = app.default_window_icon().cloned().map(|i| i.to_owned()) else {
            warn!(target: "speech", "[notify] default window icon missing, cannot restore after bounce");
            return;
        };
        for i in 0..BOUNCE_CYCLES {
            if let Err(e) = tray.set_icon(None) {
                warn!(target: "speech", "[notify] set_icon(None) failed at cycle {i}: {e}");
                break;
            }
            if play_beep {
                play_beep_async();
            }
            tokio::time::sleep(BOUNCE_STEP).await;
            if let Err(e) = tray.set_icon(Some(icon.clone())) {
                warn!(target: "speech", "[notify] set_icon(Some) failed at cycle {i}: {e}");
                break;
            }
            tokio::time::sleep(BOUNCE_STEP).await;
        }
        info!(target: "speech", "[notify] tray bounce complete");
    });
}

#[cfg(windows)]
fn play_beep_async() {
    tauri::async_runtime::spawn_blocking(|| {
        use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK;
        unsafe {
            MessageBeep(MB_ICONASTERISK);
        }
    });
}

#[cfg(not(windows))]
fn play_beep_async() {
    // No portable equivalent; this app targets Windows.
}
