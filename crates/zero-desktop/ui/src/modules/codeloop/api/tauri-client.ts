import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

// ── 与后端 agent-session / codeloop 命令对齐的类型 ──────────────────────────────

export type Provider = 'codex' | 'claude'
export type SessionStatus = 'idle' | 'generating' | 'processing' | 'unknown'

export interface SessionSummary {
  provider: Provider
  id: string
  title: string
  /** 首条用户消息前若干字符——比会被改写的 AI 标题更稳定，用于人工对照/筛选。 */
  preview: string
  /** 会话工作目录（项目路径）；前端取末段作项目名。 */
  cwd: string
  status: SessionStatus
  updated_at: string
}

export interface SessionMessage {
  role: string
  text: string
  /** 可展开详情：thinking 正文 / tool_use 入参 / tool_result 返回体（无则缺省）。 */
  detail?: string
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
  phase?: string // starting | reviewed | revised | awaiting_input | awaiting_confirm | done | error
  round?: number
  verdict?: string // pass | needs_work | parse_failed
  final_verdict?: string // pass | max_rounds | aborted_timeout | aborted_parse | aborted_by_user
  total_rounds?: number
  seq?: number
  asked_by?: Provider
  question?: AskUser
  // 逐步确认门（phase === 'awaiting_confirm'）字段：
  direction?: string // codex_to_claude | claude_to_codex
  title?: string // 确认问句
  content?: string // 即将传递的文本全文
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
  /** 逐步确认（手动）：每次跨会话传递前弹窗等用户拍板；默认 true。 */
  step_confirm?: boolean
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
  confirm: (seq: number, approve: boolean) => invoke<void>('codeloop_confirm', { seq, approve }),
  stop: () => invoke<void>('codeloop_stop'),
}

/** 订阅循环进度事件。返回值用于取消订阅。 */
export function onProgress(cb: (p: Progress) => void): Promise<UnlistenFn> {
  return listen<Progress>('codeloop_progress', e => cb(e.payload))
}
