import { RefreshCw, Plus } from 'lucide-react'
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

function optionLabel(s: SessionSummary): string {
  const title = s.title?.trim() || s.id.slice(0, 8)
  return `[${s.status}] ${title}`
}

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
  const items = sessions.filter(s => s.provider === provider)
  return (
    // min-w-0 让 flex-1 真正约束子项宽度；否则 select 会按最长 option 内容撑宽。
    <div className="flex min-w-0 flex-1 flex-col gap-1">
      <label className="text-xs text-gray-500 dark:text-gray-400">{label}</label>
      <select
        value={value}
        onChange={e => onPick(provider, e.target.value)}
        // w-full + 省略号：控件固定占满列宽，下拉展开不再因 option title 长度改变控件宽度。
        className="w-full overflow-hidden text-ellipsis whitespace-nowrap rounded-md border border-gray-300 bg-white px-2 py-1.5 text-sm outline-none focus:border-blue-400 dark:border-gray-600 dark:bg-gray-800 dark:text-gray-100"
      >
        <option value="">— 选择会话 —</option>
        {items.map(s => (
          <option key={s.id} value={s.id}>
            {optionLabel(s)}
          </option>
        ))}
      </select>
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
