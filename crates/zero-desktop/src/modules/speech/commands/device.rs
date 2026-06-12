use cpal::traits::{DeviceTrait, HostTrait};
use tauri::State;
use tracing::info;

use crate::app_state::AppState;
use crate::modules::speech::lock_utils::{read_lock, write_lock};

#[derive(serde::Serialize, Clone)]
pub struct InputDevice {
    name: String,
    is_default: bool,
}

#[tauri::command]
pub fn speech_list_input_devices() -> Result<Vec<InputDevice>, String> {
    info!(target: "speech", "[speech_list_input_devices]");
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();

    let devices: Vec<InputDevice> = host
        .input_devices()
        .map_err(|e| format!("Cannot enumerate devices: {e}"))?
        .filter_map(|d| {
            let name = d.name().ok()?;
            Some(InputDevice {
                is_default: name == default_name,
                name,
            })
        })
        .collect();

    Ok(devices)
}

#[tauri::command]
pub fn speech_set_input_device(
    device_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!(target: "speech", "[speech_set_input_device] device_name={:?}", device_name);
    let speech = state.speech.clone();
    if speech.recording.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("Cannot change device while recording".to_string());
    }
    *write_lock(&speech.selected_device) = device_name;
    Ok(())
}

#[tauri::command]
pub fn speech_get_selected_device(state: State<'_, AppState>) -> Result<Option<String>, String> {
    info!(target: "speech", "[speech_get_selected_device]");
    let speech = state.speech.clone();
    let device_name = read_lock(&speech.selected_device).clone();
    Ok(device_name)
}
