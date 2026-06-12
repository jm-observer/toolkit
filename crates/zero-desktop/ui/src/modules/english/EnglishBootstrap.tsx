/**
 * EnglishBootstrap — 播放页的前置守卫组件。
 *
 * mount 时检查：
 *   1. g10_base 是否已配置（调 english_get_g10_base）。
 *   2. customerId 是否已配置（调 EnvConfigService.getCustomerId）。
 * 任一缺失则用 react-router <Navigate> 跳转到 /settings，不渲染播放页。
 * 两者均存在则渲染 children。
 *
 * 设计约定（§4.1）：缺失就跳设置页，不静默使用默认值。
 */

import { useState, useEffect } from 'react'
import { Navigate } from 'react-router-dom'
import { invoke } from '@tauri-apps/api/core'
import { Loader2 } from 'lucide-react'
import EnvConfigService from './services/EnvConfigService'

type CheckState = 'pending' | 'ok' | 'redirect'

interface Props {
  children: React.ReactNode
}

export default function EnglishBootstrap({ children }: Props) {
  const [state, setState] = useState<CheckState>('pending')

  useEffect(() => {
    let cancelled = false
    const check = async () => {
      try {
        const g10Base = await invoke<string>('english_get_g10_base')
        if (!g10Base || !g10Base.trim()) {
          if (!cancelled) setState('redirect')
          return
        }
        const customerId = await EnvConfigService.getInstance().getCustomerId()
        if (!customerId) {
          if (!cancelled) setState('redirect')
          return
        }
        if (!cancelled) setState('ok')
      } catch (err) {
        console.error('[EnglishBootstrap] 检查失败:', err)
        if (!cancelled) setState('redirect')
      }
    }
    void check()
    return () => { cancelled = true }
  }, [])

  if (state === 'pending') {
    return (
      <div className="flex items-center justify-center gap-2 py-16 text-gray-400">
        <Loader2 size={20} className="animate-spin" />
        <span className="text-sm">检查配置...</span>
      </div>
    )
  }

  if (state === 'redirect') {
    return <Navigate to="/settings" replace />
  }

  return <>{children}</>
}
