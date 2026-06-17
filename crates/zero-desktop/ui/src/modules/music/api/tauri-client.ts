/**
 * music 模块 Tauri 客户端 —— 冻结契约。
 *
 * UI 不持有任何音频对象（无 `<audio>`），只发命令 + 听事件。命令/事件字段全部
 * snake_case，与 Rust 后端一致。所有播放真值都在后端（队列、当前曲、位置、续播）。
 */

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

// ── 类型（snake_case，与后端一致） ───────────────────────────────────────────

export interface Track {
  path: string
  title: string
  artist: string
  album: string
  duration_secs: number
  cover_path?: string
}

export type PlaybackStatus = 'playing' | 'paused' | 'stopped'
export type RepeatMode = 'off' | 'one' | 'all'

/**
 * 输出模式：
 * - `auto`   独占 bit-perfect 优先（现状），最高保真但部分设备 44.1kHz 会加速/杂音；
 * - `shared` 强制共享模式 + 重采样，兼容性好、音量可调。
 */
export type OutputMode = 'auto' | 'shared'

export interface PlaybackState {
  status: PlaybackStatus
  index: number
  track: Track | null
  position_secs: number
  duration_secs: number
  volume: number
  repeat: RepeatMode
  shuffle: boolean
}

// ── 事件载荷 ─────────────────────────────────────────────────────────────────

export interface MusicStateChanged {
  status: PlaybackStatus
  index: number
  track: Track | null
}

export interface MusicProgress {
  position_secs: number
  duration_secs: number
}

export interface MusicFormatChanged {
  sample_rate: number
  bits: number
  channels: number
  exclusive: boolean
  resampled: boolean
}

export interface MusicTrackChanged {
  index: number
  track: Track | null
}

export interface MusicError {
  message: string
}

// ── 命令（全 invoke，立即返回） ──────────────────────────────────────────────

export function musicPickFolder(): Promise<string | null> {
  return invoke<string | null>('music_pick_folder')
}

export function musicScan(dir: string): Promise<Track[]> {
  return invoke<Track[]>('music_scan', { dir })
}

export function musicPlayQueue(paths: string[], start: number): Promise<void> {
  return invoke('music_play_queue', { paths, start })
}

export function musicPause(): Promise<void> {
  return invoke('music_pause')
}

export function musicResume(): Promise<void> {
  return invoke('music_resume')
}

export function musicToggle(): Promise<void> {
  return invoke('music_toggle')
}

export function musicStop(): Promise<void> {
  return invoke('music_stop')
}

export function musicSeek(secs: number): Promise<void> {
  return invoke('music_seek', { secs })
}

export function musicNext(): Promise<void> {
  return invoke('music_next')
}

export function musicPrev(): Promise<void> {
  return invoke('music_prev')
}

export function musicSetVolume(vol: number): Promise<void> {
  return invoke('music_set_volume', { vol })
}

export function musicSetRepeat(mode: RepeatMode): Promise<void> {
  return invoke('music_set_repeat', { mode })
}

export function musicSetShuffle(on: boolean): Promise<void> {
  return invoke('music_set_shuffle', { on })
}

export function musicGetState(): Promise<PlaybackState> {
  return invoke<PlaybackState>('music_get_state')
}

export function setOutputMode(mode: OutputMode): Promise<void> {
  return invoke('music_set_output_mode', { mode })
}

// ── 事件订阅（薄封装，返回 UnlistenFn） ──────────────────────────────────────

export function onMusicStateChanged(cb: (p: MusicStateChanged) => void): Promise<UnlistenFn> {
  return listen<MusicStateChanged>('music_state_changed', e => cb(e.payload))
}

export function onMusicProgress(cb: (p: MusicProgress) => void): Promise<UnlistenFn> {
  return listen<MusicProgress>('music_progress', e => cb(e.payload))
}

export function onMusicFormatChanged(cb: (p: MusicFormatChanged) => void): Promise<UnlistenFn> {
  return listen<MusicFormatChanged>('music_format_changed', e => cb(e.payload))
}

export function onMusicTrackChanged(cb: (p: MusicTrackChanged) => void): Promise<UnlistenFn> {
  return listen<MusicTrackChanged>('music_track_changed', e => cb(e.payload))
}

export function onMusicError(cb: (p: MusicError) => void): Promise<UnlistenFn> {
  return listen<MusicError>('music_error', e => cb(e.payload))
}
