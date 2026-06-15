import { invoke } from '@tauri-apps/api/core';

// 音频清洗专属 API/类型。从 speech/api/tauri-client.ts 迁出（设计
// docs/2026-06-15-audio-clean-standalone-menu/design.md），与语音识别解耦。
// base/token 由后端自取全局 g10_base/g10_token，前端不传。

export interface CleanOptions {
  denoise?: boolean;
  pause?: string;
  separate?: boolean;
  level?: string;
  loudness?: string;
  sr?: number;
  format?: string;
}

export interface CleanedRecording {
  cleaned_path: string;
  stages: string[];
  in_lufs: number;
  out_lufs: number;
}

// 后端命令仍以 speech_ 前缀注册（历史命名，功能无关）；前端按职责归到 CleanAPI。
export const CleanAPI = {
  pickAudioFile: () => invoke<string | null>('speech_pick_audio_file'),
  openInFolder: (path: string) => invoke('speech_open_in_folder', { path }),
  cleanRecording: (inputPath: string, opts?: CleanOptions) =>
    invoke<CleanedRecording>('speech_clean_recording', { inputPath, ...opts }),
};
