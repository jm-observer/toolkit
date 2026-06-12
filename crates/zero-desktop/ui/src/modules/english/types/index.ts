/**
 * English 模块类型定义（从 shared/types 内化，无 AntD 依赖）。
 */

export interface Audio {
  id: number
  url?: string
  src?: string
  [key: string]: any
}

export interface Sentence {
  id: number
  text: string
  audios: Audio[]
  is_annotated?: boolean
  has_error?: boolean
  [key: string]: any
}

export interface EnvConfig {
  apiBaseUrl: string
  customerId?: number
  audioCoverUrl?: string
}

export type AudioPlayerEventType =
  | 'onPlayStateChange'
  | 'onStatusTextChange'
  | 'onSentenceChange'
  | 'onPlayComplete'
  | 'onPlayNext'
  | 'onPlayPrevious'
  | 'onToggleAnnotation'
  | 'onToggleReportError'
  | 'onPlayCountChange'
  | 'onTextToggle'

export interface AudioPlayerEventData {
  onPlayStateChange: { isPlaying: boolean }
  onStatusTextChange: { statusText: string }
  onSentenceChange: { sentences: Sentence[]; currentSentenceIndex: number }
  onPlayComplete: Record<string, never>
  onPlayNext: { sentenceId?: number; sentenceIndex: number; sentence: Sentence }
  onPlayPrevious: { sentenceId?: number; sentenceIndex: number; sentence: Sentence }
  onToggleAnnotation: { sentenceId: number; isAnnotated: boolean; sentence: Sentence }
  onToggleReportError: { sentenceId: number; hasError: boolean; sentence: Sentence }
  onPlayCountChange: {
    playCount: number
    currentSentenceIndex: number
    currentAudioIndex: number
    maxPlayCount: number
  }
  onTextToggle: { showText: boolean }
}

export interface AudioPlayerState {
  isPlaying: boolean
  currentSentenceIndex: number
  currentAudioIndex: number
  playCount: number
  maxPlayCount: number
  stopMode: 'halfHour' | 'roundEnd' | null
  statusText: string
  sentences: Sentence[]
  currentSentence: Sentence | null
  currentAudio: Audio | null
  showText?: boolean
}

export interface CacheItem {
  key: string
  filePath: string
  url: string
  size: number
  createdAt: number
}

export interface CacheStats {
  totalSize: number
  totalCount: number
  items: CacheItem[]
}

export interface ApiRequest {
  method: string
  params?: Record<string, any>
}

export interface ApiResponse<T = any> {
  code: number
  msg: string
  data: T
}
