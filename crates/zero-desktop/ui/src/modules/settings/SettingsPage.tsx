/**
 * SettingsPage — 统一设置页（阶段 4 扩充）。
 *
 * 新增：
 * - G10 配置区（调 cookie_get_app_settings / cookie_save_app_settings）。
 * - 英语模块区（嵌入 EnvConfig 组件，管理 customer_id）。
 */

import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { Save, CheckCircle, XCircle } from 'lucide-react'
import EnvConfig from '../english/components/EnvConfig'
import { Button } from '../speech/components/ui/Button'

// ── 主题 ─────────────────────────────────────────────────────────────────────

function getStoredTheme(): 'light' | 'dark' {
  return (localStorage.getItem('theme') as 'light' | 'dark') ?? 'light'
}

function applyTheme(theme: 'light' | 'dark') {
  if (theme === 'dark') document.documentElement.classList.add('dark')
  else document.documentElement.classList.remove('dark')
  localStorage.setItem('theme', theme)
}

// ── G10 配置 ──────────────────────────────────────────────────────────────────

interface AppSettings {
  g10_base: string
  g10_token?: string | null
}

function G10ConfigSection() {
  const [g10Base, setG10Base] = useState('')
  const [g10Token, setG10Token] = useState('')
  const [feedback, setFeedback] = useState<{ kind: 'ok' | 'err'; msg: string } | null>(null)
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    void invoke<AppSettings>('cookie_get_app_settings').then(s => {
      setG10Base(s.g10_base ?? '')
      setG10Token(s.g10_token ?? '')
    }).catch(err => console.error('[SettingsPage] 加载 G10 配置失败:', err))
  }, [])

  const showFeedback = (kind: 'ok' | 'err', msg: string) => {
    setFeedback({ kind, msg })
    setTimeout(() => setFeedback(null), 3000)
  }

  const handleSave = async () => {
    setLoading(true)
    try {
      const settings: AppSettings = {
        g10_base: g10Base.trim(),
        g10_token: g10Token.trim() || null
      }
      await invoke('cookie_save_app_settings', { settings })
      showFeedback('ok', 'G10 配置已保存')
    } catch (err: any) {
      showFeedback('err', '保存失败: ' + (err?.message ?? String(err)))
    } finally {
      setLoading(false)
    }
  }

  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-sm font-medium text-gray-600 dark:text-gray-400">G10 配置</h2>

      {feedback && (
        <div className={[
          'flex items-center gap-2 rounded-md px-3 py-2 text-xs',
          feedback.kind === 'ok'
            ? 'bg-green-50 text-green-700 dark:bg-green-900/20 dark:text-green-400'
            : 'bg-red-50 text-red-600 dark:bg-red-900/20 dark:text-red-400'
        ].join(' ')}>
          {feedback.kind === 'ok' ? <CheckCircle size={13} /> : <XCircle size={13} />}
          {feedback.msg}
        </div>
      )}

      <div className="flex flex-col gap-2">
        <label className="text-xs text-gray-500 dark:text-gray-400">G10 base URL</label>
        <input
          type="text"
          value={g10Base}
          onChange={e => setG10Base(e.target.value)}
          placeholder="http://192.168.1.100:8788"
          className="rounded-md border border-gray-300 bg-white px-3 py-1.5 text-sm outline-none focus:border-blue-400 dark:border-gray-600 dark:bg-gray-800 dark:text-gray-100"
        />
      </div>

      <div className="flex flex-col gap-2">
        <label className="text-xs text-gray-500 dark:text-gray-400">G10 Bearer Token（可选）</label>
        <input
          type="password"
          value={g10Token}
          onChange={e => setG10Token(e.target.value)}
          placeholder="留空表示不鉴权"
          className="rounded-md border border-gray-300 bg-white px-3 py-1.5 text-sm outline-none focus:border-blue-400 dark:border-gray-600 dark:bg-gray-800 dark:text-gray-100"
        />
      </div>

      <Button variant="primary" size="sm" disabled={loading} onClick={handleSave} className="self-start">
        <Save size={14} />
        {loading ? '保存中...' : '保存 G10 配置'}
      </Button>
    </section>
  )
}

// ── 主组件 ────────────────────────────────────────────────────────────────────

export default function SettingsPage() {
  const [theme, setTheme] = useState<'light' | 'dark'>(getStoredTheme)

  useEffect(() => { applyTheme(theme) }, [theme])

  return (
    <div className="flex flex-col gap-8 max-w-xl">
      <h1 className="text-xl font-semibold">设置</h1>

      {/* 外观 */}
      <section className="flex flex-col gap-3">
        <h2 className="text-sm font-medium text-gray-600 dark:text-gray-400">外观</h2>
        <div className="flex items-center gap-3">
          <span className="text-sm">主题</span>
          <button
            onClick={() => setTheme(prev => prev === 'light' ? 'dark' : 'light')}
            className="rounded-md border border-gray-300 px-4 py-1.5 text-sm hover:bg-gray-100 dark:border-gray-600 dark:hover:bg-gray-800"
          >
            {theme === 'light' ? '切换深色' : '切换浅色'}
          </button>
          <span className="text-xs text-gray-400">当前：{theme === 'light' ? '浅色' : '深色'}</span>
        </div>
      </section>

      {/* G10 配置 */}
      <G10ConfigSection />

      {/* 英语模块 */}
      <section className="flex flex-col gap-3">
        <h2 className="text-sm font-medium text-gray-600 dark:text-gray-400">英语模块</h2>
        <EnvConfig />
      </section>
    </div>
  )
}
