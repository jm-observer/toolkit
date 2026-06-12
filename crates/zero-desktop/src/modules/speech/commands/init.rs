use tauri::State;

use crate::app_state::AppState;
use crate::modules::speech::lock_utils::read_lock;

#[derive(serde::Serialize, Clone)]
pub struct InitStatus {
    status: u8,
    error: String,
}

// Polled ~1/s by the frontend — intentionally no logging (would spam).
#[tauri::command]
pub fn speech_get_init_status(state: State<'_, AppState>) -> Result<InitStatus, String> {
    let speech = state.speech.clone();
    let status = speech
        .init_status
        .load(std::sync::atomic::Ordering::Relaxed);
    let error = read_lock(&speech.init_error).clone();
    Ok(InitStatus { status, error })
}
