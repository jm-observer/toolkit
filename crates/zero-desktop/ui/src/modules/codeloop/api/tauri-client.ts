import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

// ── 与后端 agent-session / codeloop 命令对齐的类型 ──────────────────────────────

export type Provider = 'codex' | 'claude'
export type SessionStatus = 'idle' | 'generating' | 'processing' | 'unknown'

export interface SessionSummary {
  provider: Provider
  id: string
  title: string
  status: SessionStatus
  updated_at: string
}

export interface SessionMessage {
  role: string
  text: string
  timestamp: string
}

export interface MessagesPage {
  messages: SessionMessage[]
  cursor: number
}

export interface AskUser {
  question: string
  options?: string[]
}

/** 循环进度（codeloop://progress 事件载荷，字段随 phase 变化）。 */
export interface Progress {
  phase?: string // starting | reviewed | revised | awaiting_input | done | error
  round?: number
  verdict?: string // pass | needs_work | parse_failed
  final_verdict?: string // pass | max_rounds | aborted_timeout | aborted_parse
  total_rounds?: number
  seq?: number
  asked_by?: Provider
  question?: AskUser
  error?: string
}

export interface StatusSnapshot {
  running: boolean
  progress: Progress | null
}

export type ReviewMode = 'design' | 'implementation'

export interface StartInput {
  claude: { session_id: string; cwd?: string }
  codex: { session_id: string; cwd?: string }
  target_path: string
  target_label?: string
  mode: ReviewMode
  max_rounds?: number
  wait_for_claude_idle?: boolean
}

export const CodeloopAPI = {
  listSessions: (limit = 30) => invoke<SessionSummary[]>('codeloop_list_sessions', { limit }),
  sessionMessages: (provider: Provider, sessionId: string, after: number) =>
    invoke<MessagesPage>('codeloop_session_messages', { provider, sessionId, after }),
  newCodexSession: (claudeSessionId: string) =>
    invoke<string>('codeloop_new_codex_session', { claudeSessionId }),
  start: (input: StartInput) => invoke<void>('codeloop_start', { input }),
  status: () => invoke<StatusSnapshot>('codeloop_status'),
  answer: (seq: number, text: string) => invoke<void>('codeloop_answer', { seq, text }),
  stop: () => invoke<void>('codeloop_stop'),
}

/** 订阅循环进度事件。返回值用于取消订阅。 */
export function onProgress(cb: (p: Progress) => void): Promise<UnlistenFn> {
  return listen<Progress>('codeloop_progress', e => cb(e.payload))
}
