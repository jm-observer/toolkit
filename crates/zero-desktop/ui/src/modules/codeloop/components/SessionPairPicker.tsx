import { useEffect, useMemo, useRef, useState } from 'react'
import { RefreshCw, Plus, ChevronDown } from 'lucide-react'
import type { Provider, SessionSummary } from '../api/tauri-client'

interface Props {
  sessions: SessionSummary[]
  claudeId: string
  codexId: string
  onPick: (provider: Provider, id: string) => void
  onRefresh: () => void
  loading: boolean
  /** 新建 Codex 会话（复用所选 Claude 会话的仓库目录）。 */
  onNewCodex: () => void
  creatingCodex: boolean
}

/** 主展示文本：优先首条用户消息预览（最稳定可认），回退 AI 标题，再回退短 id。 */
function optionLabel(s: SessionSummary): string {
  const body = s.preview?.trim() || s.title?.trim() || s.id.slice(0, 8)
  return `[${s.status}] ${body}`
}

/** 项目名：取 cwd 末段（兼容 / 与 \\ 分隔）；无则空串。 */
function projectName(cwd: string): string {
  const parts = (cwd || '').split(/[/\\]+/).filter(Boolean)
  return parts.length ? parts[parts.length - 1] : ''
}

/** 分词匹配：query 拆成词，每个词都需在 haystack 中出现（顺序无关、部分匹配）。 */
function tokenMatch(haystack: string, query: string): boolean {
  const hay = haystack.toLowerCase()
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every(tok => hay.includes(tok))
}

/** updated_at(ISO8601) → 本地化短时间；无法解析则原样返回。 */
function shortTime(iso: string): string {
  const t = Date.parse(iso)
  if (Number.isNaN(t)) return iso
  return new Date(t).toLocaleString(undefined, {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

/**
 * 可输入过滤的会话下拉框（替代原生 select）。
 * - 输入框即筛选框：键入文本按 title/id/status 模糊匹配。
 * - 选项按 `updated_at` 倒序（最新在最前）。
 * - 点击选项选中；Esc / 点击外部关闭；回车选中当前列表首项。
 */
function SideSelect({
  label,
  provider,
  sessions,
  value,
  onPick,
}: {
  label: string
  provider: Provider
  sessions: SessionSummary[]
  value: string
  onPick: (provider: Provider, id: string) => void
}) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const rootRef = useRef<HTMLDivElement>(null)

  // 仅本侧 provider，按 updated_at 倒序（最新在最前）。
  const sorted = useMemo(
    () =>
      sessions
        .filter(s => s.provider === provider)
        .slice()
        .sort((a, b) => (a.updated_at < b.updated_at ? 1 : a.updated_at > b.updated_at ? -1 : 0)),
    [sessions, provider],
  )

  const selected = sorted.find(s => s.id === value)

  // 打开且已键入时按 query 分词过滤（跨 预览/标题/项目/状态/id）；否则展示全部（倒序）。
  const filtered = useMemo(() => {
    const q = query.trim()
    if (!open || !q) return sorted
    return sorted.filter(s =>
      tokenMatch(`${s.preview} ${s.title} ${projectName(s.cwd)} ${s.status} ${s.id}`, q),
    )
  }, [sorted, query, open])

  // 点击组件外部关闭下拉。
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [open])

  const pick = (id: string) => {
    onPick(provider, id)
    setOpen(false)
    setQuery('')
  }

  // 关闭态展示已选会话标签；打开态展示用户输入。
  const display = open ? query : selected ? optionLabel(selected) : ''

  return (
    // min-w-0 让 flex-1 真正约束子项宽度。
    <div ref={rootRef} className="relative flex min-w-0 flex-1 flex-col gap-1">
      <label className="text-xs text-gray-500 dark:text-gray-400">{label}</label>
      <div className="relative">
        <input
          value={display}
          placeholder="— 选择 / 输入筛选 —"
          onChange={e => {
            setQuery(e.target.value)
            if (!open) setOpen(true)
          }}
          onFocus={() => {
            setOpen(true)
            setQuery('')
          }}
          onKeyDown={e => {
            if (e.key === 'Escape') {
              setOpen(false)
              ;(e.target as HTMLInputElement).blur()
            } else if (e.key === 'Enter' && open && filtered.length > 0) {
              e.preventDefault()
              pick(filtered[0].id)
              ;(e.target as HTMLInputElement).blur()
            }
          }}
          className="w-full overflow-hidden text-ellipsis whitespace-nowrap rounded-md border border-gray-300 bg-white px-2 py-1.5 pr-7 text-sm outline-none focus:border-blue-400 dark:border-gray-600 dark:bg-gray-800 dark:text-gray-100"
        />
        <ChevronDown
          size={14}
          className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 text-gray-400"
        />
        {open && (
          <ul className="absolute left-0 top-full z-20 mt-1 max-h-64 w-max min-w-full max-w-[32rem] overflow-auto rounded-md border border-gray-200 bg-white py-1 shadow-lg dark:border-gray-600 dark:bg-gray-800">
            <li
              // mousedown 而非 click：抢在 input blur 之前触发，避免列表先被关闭。
              onMouseDown={e => {
                e.preventDefault()
                pick('')
              }}
              className="cursor-pointer px-2 py-1.5 text-sm text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700"
            >
              — 清除选择 —
            </li>
            {filtered.length === 0 && (
              <li className="px-2 py-1.5 text-sm text-gray-400">无匹配会话</li>
            )}
            {filtered.map(s => (
              <li
                key={s.id}
                onMouseDown={e => {
                  e.preventDefault()
                  pick(s.id)
                }}
                className={`flex cursor-pointer items-center justify-between gap-2 px-2 py-1.5 text-sm hover:bg-gray-100 dark:hover:bg-gray-700 ${
                  s.id === value ? 'bg-blue-50 dark:bg-gray-700/60' : ''
                }`}
              >
                <span
                  className="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap"
                  title={s.title || undefined}
                >
                  {optionLabel(s)}
                </span>
                <span className="flex shrink-0 items-center gap-1.5 text-xs text-gray-400">
                  {projectName(s.cwd) && (
                    <span className="rounded bg-gray-100 px-1 py-0.5 text-gray-500 dark:bg-gray-700 dark:text-gray-300">
                      {projectName(s.cwd)}
                    </span>
                  )}
                  {shortTime(s.updated_at)}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  )
}

export function SessionPairPicker({
  sessions,
  claudeId,
  codexId,
  onPick,
  onRefresh,
  loading,
  onNewCodex,
  creatingCodex,
}: Props) {
  return (
    <div className="flex items-end gap-3">
      <SideSelect label="Claude Code 会话" provider="claude" sessions={sessions} value={claudeId} onPick={onPick} />
      <SideSelect label="Codex 会话" provider="codex" sessions={sessions} value={codexId} onPick={onPick} />
      <button
        onClick={onNewCodex}
        disabled={creatingCodex || !claudeId}
        title={claudeId ? '新建 Codex 会话（复用所选 Claude 会话的仓库目录，消耗 codex 额度）' : '请先选择 Claude 会话以确定仓库目录'}
        className="flex items-center gap-1.5 rounded-md border border-gray-300 px-3 py-1.5 text-sm hover:bg-gray-100 disabled:opacity-50 dark:border-gray-600 dark:hover:bg-gray-800"
      >
        <Plus size={14} className={creatingCodex ? 'animate-spin' : ''} />
        {creatingCodex ? '新建中…' : '新建 Codex'}
      </button>
      <button
        onClick={onRefresh}
        disabled={loading}
        title="刷新会话清单"
        className="flex items-center gap-1.5 rounded-md border border-gray-300 px-3 py-1.5 text-sm hover:bg-gray-100 disabled:opacity-50 dark:border-gray-600 dark:hover:bg-gray-800"
      >
        <RefreshCw size={14} className={loading ? 'animate-spin' : ''} />
        刷新
      </button>
    </div>
  )
}
