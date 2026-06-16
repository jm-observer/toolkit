/**
 * ChatSummaryPage — 对话总结。
 *
 * 粘贴一段会话文本 → 调 G10 toolkit-server 的 `/api/web/llm/summarize`（经 llm_summarize
 * Tauri 命令，用可配的 chat_summary 提示词 + 公共大模型）→ 展示要点总结。
 */

import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { Sparkles, Copy, CheckCircle, XCircle } from 'lucide-react'

export default function ChatSummaryPage() {
  const [text, setText] = useState('')
  const [summary, setSummary] = useState('')
  const [model, setModel] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)

  const handleSummarize = async () => {
    if (!text.trim()) {
      setError('请先粘贴会话内容')
      return
    }
    setLoading(true)
    setError(null)
    setSummary('')
    try {
      const r = await invoke<{ summary: string; model: string }>('llm_summarize', { text })
      setSummary(r.summary ?? '')
      setModel(r.model ?? '')
    } catch (e: any) {
      setError(typeof e === 'string' ? e : (e?.message ?? String(e)))
    } finally {
      setLoading(false)
    }
  }

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(summary)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      /* 忽略剪贴板失败 */
    }
  }

  return (
    <div className="flex flex-col gap-4 max-w-3xl">
      <div>
        <h1 className="text-xl font-semibold">对话总结</h1>
        <p className="mt-1 text-xs text-gray-500 dark:text-gray-400">
          粘贴会话文本，调用公共大模型输出要点总结。提示词可在「设置 → 大模型」里调整（chat_summary）。
        </p>
      </div>

      <textarea
        value={text}
        onChange={e => setText(e.target.value)}
        placeholder="把要总结的对话 / 文本粘贴到这里…"
        rows={12}
        className="w-full resize-y rounded-md border border-gray-300 bg-white px-3 py-2 text-sm outline-none focus:border-blue-400 dark:border-gray-600 dark:bg-gray-800 dark:text-gray-100"
      />

      <div className="flex items-center gap-3">
        <button
          type="button"
          onClick={() => void handleSummarize()}
          disabled={loading}
          className="flex items-center gap-2 rounded-md bg-blue-500 px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-blue-600 disabled:opacity-60"
        >
          <Sparkles size={15} />
          {loading ? '总结中…' : '生成总结'}
        </button>
        <span className="text-xs text-gray-400">{text.length} 字</span>
      </div>

      {error && (
        <div className="flex items-start gap-2 rounded-md bg-red-50 px-3 py-2 text-xs text-red-600 dark:bg-red-900/20 dark:text-red-400">
          <XCircle size={13} className="mt-0.5 flex-shrink-0" />
          <span>{error}</span>
        </div>
      )}

      {summary && (
        <section className="flex flex-col gap-2 rounded-md border border-gray-200 bg-gray-50 p-4 dark:border-gray-700 dark:bg-gray-900">
          <div className="flex items-center justify-between">
            <h2 className="text-sm font-medium text-gray-600 dark:text-gray-400">
              总结{model ? ` · ${model}` : ''}
            </h2>
            <button
              type="button"
              onClick={() => void handleCopy()}
              className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-gray-500 hover:bg-gray-200 dark:text-gray-400 dark:hover:bg-gray-800"
            >
              {copied ? <CheckCircle size={12} /> : <Copy size={12} />}
              {copied ? '已复制' : '复制'}
            </button>
          </div>
          <pre className="whitespace-pre-wrap break-words font-sans text-sm leading-relaxed text-gray-800 dark:text-gray-200">
            {summary}
          </pre>
        </section>
      )}
    </div>
  )
}
