import { X } from 'lucide-react'
import type { SessionMessage } from '../api/tauri-client'
import { MessageColumn } from './MessageColumn'

interface Props {
  claudeId: string
  codexId: string
  claudeMessages: SessionMessage[]
  codexMessages: SessionMessage[]
  onClose: () => void
}

/** 跟踪弹窗：并排展示所选记录两个会话（Claude / Codex）的消息记录（实时增量轮询）。 */
export function TrackModal(props: Props) {
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
      onClick={props.onClose}
    >
      <div
        className="flex h-[85vh] w-[92vw] max-w-6xl flex-col rounded-lg bg-white shadow-xl dark:bg-gray-900"
        onClick={e => e.stopPropagation()}
      >
        <div className="flex items-center justify-between border-b border-gray-200 px-4 py-2 dark:border-gray-800">
          <span className="text-sm font-medium text-gray-700 dark:text-gray-200">
            跟踪会话消息（实时）
          </span>
          <button
            onClick={props.onClose}
            className="rounded p-1 text-gray-400 hover:bg-gray-100 hover:text-gray-700 dark:hover:bg-gray-800"
            title="关闭"
          >
            <X size={16} />
          </button>
        </div>
        <div className="flex min-h-0 flex-1 gap-3 p-3">
          <MessageColumn title="Claude Code" sessionId={props.claudeId} messages={props.claudeMessages} />
          <MessageColumn title="Codex" sessionId={props.codexId} messages={props.codexMessages} />
        </div>
      </div>
    </div>
  )
}
