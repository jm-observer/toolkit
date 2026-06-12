use tauri::State;

use crate::app_state::AppState;
use crate::modules::speech::settings::{
    apply_settings_to_state, get_settings_from_state, CombinedSettings,
};

#[tauri::command]
pub fn speech_get_settings(state: State<'_, AppState>) -> Result<CombinedSettings, String> {
    let speech = state.speech.clone();
    get_settings_from_state(&speech)
}

#[tauri::command]
pub async fn speech_apply_settings(
    new_settings: CombinedSettings,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let speech = state.speech.clone();
    apply_settings_to_state(new_settings, &speech).await
}
