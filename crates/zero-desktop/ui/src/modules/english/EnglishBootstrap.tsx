/**
 * EnglishBootstrap — 播放页的前置守卫组件。
 *
 * mount 时检查：
 *   1. g10_base 是否已配置（调 english_get_g10_base）。
 *   2. customerId 是否已配置（调 EnvConfigService.getCustomerId）。
 *
 * 任一缺失则在英语页面内显示「需要配置」提示卡片（含「去设置页」按钮），
 * 不渲染播放页；两者均存在则渲染 children。
 *
 * 设计约定（§4.1）：缺失就不进播放页，且引导用户去设置——但不硬跳，避免
 * 「点英语听力却莫名其妙跳到设置」的困惑。
 */

import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { invoke } from '@tauri-apps/api/core'
import { Loader2, AlertCircle, Settings } from 'lucide-react'
import EnvConfigService from './services/EnvConfigService'

type CheckState =
  | { kind: 'pending' }
  | { kind: 'ok' }
  | { kind: 'missing'; reasons: string[] }

interface Props {
  children: React.ReactNode
}

export default function EnglishBootstrap({ children }: Props) {
  const [state, setState] = useState<CheckState>({ kind: 'pending' })
  const navigate = useNavigate()

  useEffect(() => {
    let cancelled = false
    const check = async () => {
      const reasons: string[] = []
      try {
        const g10Base = await invoke<string>('english_get_g10_base')
        if (!g10Base || !g10Base.trim()) reasons.push('G10 base 未配置')
      } catch (err) {
        console.error('[EnglishBootstrap] g10_base 检查失败:', err)
        reasons.push('无法读取 G10 base 配置')
      }
      try {
        const customerId = await EnvConfigService.getInstance().getCustomerId()
        if (!customerId) reasons.push('customer_id 未配置')
      } catch (err) {
        console.error('[EnglishBootstrap] customer_id 检查失败:', err)
        reasons.push('无法读取 customer_id')
      }
      if (cancelled) return
      setState(reasons.length === 0 ? { kind: 'ok' } : { kind: 'missing', reasons })
    }
    void check()
    return () => { cancelled = true }
  }, [])

  if (state.kind === 'pending') {
    return (
      <div className="flex items-center justify-center gap-2 py-16 text-gray-400">
        <Loader2 size={20} className="animate-spin" />
        <span className="text-sm">检查配置...</span>
      </div>
    )
  }

  if (state.kind === 'missing') {
    return (
      <div className="mx-auto mt-16 max-w-lg rounded-lg border border-amber-200 bg-amber-50 p-6 shadow-sm">
        <div className="mb-3 flex items-center gap-2 text-amber-700">
          <AlertCircle size={20} />
          <h2 className="text-base font-semibold">英语模块需要先完成配置</h2>
        </div>
        <ul className="mb-4 ml-7 list-disc space-y-1 text-sm text-amber-800">
          {state.reasons.map((r) => <li key={r}>{r}</li>)}
        </ul>
        <button
          type="button"
          onClick={() => navigate('/settings')}
          className="inline-flex items-center gap-2 rounded-md bg-amber-600 px-4 py-2 text-sm font-medium text-white hover:bg-amber-700"
        >
          <Settings size={16} />
          去设置页配置
        </button>
      </div>
    )
  }

  return <>{children}</>
}
