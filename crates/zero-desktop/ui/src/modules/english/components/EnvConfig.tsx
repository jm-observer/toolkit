/**
 * EnvConfig — 英语模块环境配置组件（Tailwind 重写，无 AntD）。
 *
 * 改造点：
 * - 删除 dev/prod 切换（apiBase 统一来自 g10_base，不再独立配置）。
 * - 只管理 customer_id（存 plugin-store）。
 * - G10 base/token 显示（只读，编辑入口在父级设置页）。
 * - 无 AntD message —— 用内联状态反馈替代。
 */

import { useState, useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { Save, Trash2, CheckCircle, XCircle } from 'lucide-react'
import EnvConfigService from '../services/EnvConfigService'
import { Button } from '../../speech/components/ui/Button'

export default function EnvConfig() {
  const [g10Base, setG10Base] = useState<string>('')
  const [customerId, setCustomerId] = useState<number | undefined>(undefined)
  const [customerIdInput, setCustomerIdInput] = useState('')
  const [feedback, setFeedback] = useState<{ kind: 'ok' | 'err'; msg: string } | null>(null)

  useEffect(() => {
    void loadInfo()
  }, [])

  const loadInfo = async () => {
    try {
      const base = await invoke<string>('english_get_g10_base')
      setG10Base(base || '')
      const cid = await EnvConfigService.getInstance().getCustomerId()
      setCustomerId(cid)
      setCustomerIdInput(cid?.toString() ?? '')
    } catch (error) {
      console.error('[EnvConfig] 加载失败:', error)
    }
  }

  const showFeedback = (kind: 'ok' | 'err', msg: string) => {
    setFeedback({ kind, msg })
    setTimeout(() => setFeedback(null), 3000)
  }

  const handleSaveCustomerId = async () => {
    const id = customerIdInput ? parseInt(customerIdInput, 10) : NaN
    if (!id || id <= 0) { showFeedback('err', '请输入有效的 Customer ID（正整数）'); return }
    const ok = await EnvConfigService.getInstance().setCustomerId(id)
    if (ok) {
      setCustomerId(id)
      showFeedback('ok', 'Customer ID 已保存')
      window.dispatchEvent(new CustomEvent('customer-id-changed'))
    } else {
      showFeedback('err', '保存失败，请重试')
    }
  }

  const handleClearCustomerId = async () => {
    const ok = await EnvConfigService.getInstance().clearCustomerId()
    if (ok) {
      setCustomerId(undefined)
      setCustomerIdInput('')
      showFeedback('ok', 'Customer ID 已清除')
      window.dispatchEvent(new CustomEvent('customer-id-changed'))
    } else {
      showFeedback('err', '清除失败，请重试')
    }
  }

  return (
    <div className="flex flex-col gap-5">
      {/* 反馈提示 */}
      {feedback && (
        <div className={[
          'flex items-center gap-2 rounded-md px-3 py-2 text-sm',
          feedback.kind === 'ok'
            ? 'bg-green-50 text-green-700 dark:bg-green-900/20 dark:text-green-400'
            : 'bg-red-50 text-red-600 dark:bg-red-900/20 dark:text-red-400'
        ].join(' ')}>
          {feedback.kind === 'ok' ? <CheckCircle size={14} /> : <XCircle size={14} />}
          {feedback.msg}
        </div>
      )}

      {/* G10 地址（只读，告知来源） */}
      <section className="flex flex-col gap-2">
        <h3 className="text-sm font-medium text-gray-600 dark:text-gray-400">G10 API 地址</h3>
        <div className="rounded-md border border-gray-200 bg-gray-50 px-3 py-2 text-sm text-gray-700 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-300">
          {g10Base || <span className="text-yellow-600 dark:text-yellow-400">未配置，请到上方「G10 配置」区域设置</span>}
        </div>
        <p className="text-xs text-gray-400">此地址由全局设置中的「G10 base」决定，不可在此处单独修改。</p>
      </section>

      {/* Customer ID */}
      <section className="flex flex-col gap-3">
        <h3 className="text-sm font-medium text-gray-600 dark:text-gray-400">Customer ID</h3>
        <div className="flex items-center gap-2 text-sm">
          <span className="text-gray-500 dark:text-gray-400">当前：</span>
          <span className={customerId ? 'font-mono text-gray-900 dark:text-gray-100' : 'text-yellow-600 dark:text-yellow-400'}>
            {customerId ?? '未设置'}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <input
            type="number"
            min={1}
            step={1}
            value={customerIdInput}
            onChange={e => setCustomerIdInput(e.target.value)}
            placeholder="请输入 Customer ID"
            className="w-44 rounded-md border border-gray-300 bg-white px-3 py-1.5 text-sm outline-none focus:border-blue-400 dark:border-gray-600 dark:bg-gray-800 dark:text-gray-100"
          />
          <Button variant="primary" size="sm" onClick={handleSaveCustomerId}>
            <Save size={14} />
            保存
          </Button>
          {customerId && (
            <Button variant="outline" size="sm" onClick={handleClearCustomerId}>
              <Trash2 size={14} />
              清除
            </Button>
          )}
        </div>
        <p className="text-xs text-gray-400">customer_id 用于个性化标注和报错，存储在本地 plugin-store。</p>
      </section>
    </div>
  )
}
