import { invoke } from '@tauri-apps/api/core';

export interface Segment {
  id: number | null;
  segment_id?: number | null;
  revision?: number;
  start: number;
  end: number;
  wall_start: string;
  wall_end: string;
  text_raw: string;
  text_optimized?: string;
  text_english?: string;
  text_secondary?: string;
  secondary_kind?: string;
  speaker?: string;
  optimize_status: 'pending' | 'running' | 'success' | 'failed';
  translate_status: 'blocked' | 'pending' | 'running' | 'success' | 'failed';
}

export interface RecordingState {
  recording: boolean;
}

export interface InputDeviceInfo {
  name: string;
  is_default: boolean;
}

export interface InitStatus {
  status: number;
  error?: string;
}

export type AsrLanguage = '' | 'zh' | 'en' | 'ja' | 'ko' | 'yue';
export type AutoCopyMode = 'off' | 'english' | 'optimized_zh';

export interface AppSettings {
  asr_language: AsrLanguage;
  auto_copy_mode: AutoCopyMode;
  merge_window_ms: number;
  remote_url: string;
  remote_url_presets: string[];
  want_secondary: boolean;
  notify_sound: boolean;
}

export const DEFAULT_REMOTE_URL = 'ws://192.168.0.68:8090/stream';

export interface SegmentDiscardedEvent {
  revision: number;
  segment_id: number;
  decision: 'DISCARD';
  reason: string;
  source: 'rule' | 'llm';
  confidence: number | null;
  occurred_at_ms: number;
}

export interface SegmentUpdatedEvent {
  id: number;
  segment_id: number;
  revision: number;
  start_sec: number;
  end_sec: number;
  wall_start: string;
  wall_end: string;
  text_raw: string;
  optimize_status: Segment['optimize_status'];
  translate_status: Segment['translate_status'];
  text_optimized?: string;
  text_english?: string;
  text_secondary?: string;
  secondary_kind?: string;
  speaker?: string;
  created_at: string;
}

// All commands prefixed with speech_ to match zero-desktop backend naming.
export const SpeechAPI = {
  startRecording: () => invoke('speech_start_recording'),
  stopRecording: () => invoke('speech_stop_recording'),
  getRecordingState: () => invoke<RecordingState>('speech_get_recording_state'),
  fetchRemoteHistory: (limit: number) =>
    invoke<Record<string, unknown>[]>('speech_fetch_remote_history', { limit }),
  listDevices: () => invoke<InputDeviceInfo[]>('speech_list_input_devices'),
  getSelectedDevice: () => invoke<string | null>('speech_get_selected_device'),
  setInputDevice: (deviceName: string | null) =>
    invoke('speech_set_input_device', { deviceName }),
  getInitStatus: () => invoke<InitStatus>('speech_get_init_status'),
  clearResults: () => invoke('speech_clear_results'),
  copyToClipboard: (text: string) =>
    invoke('speech_copy_text_to_clipboard', { text }),
  getSettings: () => invoke<AppSettings>('speech_get_settings'),
  applySettings: (newSettings: AppSettings) =>
    invoke('speech_apply_settings', { newSettings }),
};
