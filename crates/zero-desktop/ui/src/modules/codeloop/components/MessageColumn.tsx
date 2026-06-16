import { useEffect, useRef } from 'react'
import type { SessionMessage } from '../api/tauri-client'

interface Props {
  title: string
  sessionId: string
  messages: SessionMessage[]
}

/** 纯工具调用 / 思考标记的消息折叠展示（后端已渲染成 `[tool_use: ..]` / `[thinking]`）。 */
function isFoldable(text: string): boolean {
  return /^\[(tool_use|tool_result|thinking)\b.*\]$/.test(text.trim())
}

function Bubble({ msg }: { msg: SessionMessage }) {
  // 有详情（thinking 正文 / tool_use 入参 / tool_result 返回体）：可展开看正文。
  if (msg.detail) {
    return (
      <details className="my-1 text-xs text-gray-500 dark:text-gray-400">
        <summary className="cursor-pointer select-none">🔧 {msg.text}</summary>
        <pre className="mt-1 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded bg-gray-50 p-2 font-mono text-[11px] leading-relaxed text-gray-600 dark:bg-gray-800/60 dark:text-gray-300">
          {msg.detail}
        </pre>
      </details>
    )
  }
  // 纯标记但无正文可展开：渲染成一行灰字，不画误导的展开三角。
  if (isFoldable(msg.text)) {
    return (
      <div className="my-1 select-none text-xs text-gray-400 dark:text-gray-500">🔧 {msg.text}</div>
    )
  }
  const isUser = msg.role === 'user'
  return (
    <div className={`my-1.5 flex ${isUser ? 'justify-end' : 'justify-start'}`}>
      <div
        className={[
          'max-w-[85%] whitespace-pre-wrap break-words rounded-lg px-3 py-2 text-sm',
          isUser
            ? 'bg-blue-100 text-blue-900 dark:bg-blue-900/40 dark:text-blue-100'
            : 'bg-gray-100 text-gray-800 dark:bg-gray-800 dark:text-gray-100',
        ].join(' ')}
      >
        {msg.text}
      </div>
    </div>
  )
}

export function MessageColumn({ title, sessionId, messages }: Props) {
  const ref = useRef<HTMLDivElement>(null)
  const stick = useRef(true)

  // 用户上滚时暂停自动滚动；滚回接近底部恢复。
  const onScroll = () => {
    const el = ref.current
    if (!el) return
    stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60
  }

  useEffect(() => {
    const el = ref.current
    if (el && stick.current) el.scrollTop = el.scrollHeight
  }, [messages])

  return (
    <div className="flex min-h-0 flex-1 flex-col rounded-md border border-gray-200 dark:border-gray-800">
      <div className="flex items-center justify-between border-b border-gray-200 px-3 py-1.5 text-xs font-medium text-gray-600 dark:border-gray-800 dark:text-gray-300">
        <span>{title}</span>
        <span className="text-gray-400">{sessionId ? sessionId.slice(0, 8) + '…' : '未选择'}</span>
      </div>
      <div ref={ref} onScroll={onScroll} className="min-h-0 flex-1 overflow-auto px-3 py-2">
        {messages.length === 0 ? (
          <div className="pt-8 text-center text-xs text-gray-400">
            {sessionId ? '暂无消息' : '请选择会话'}
          </div>
        ) : (
          messages.map((m, i) => <Bubble key={i} msg={m} />)
        )}
      </div>
    </div>
  )
}
