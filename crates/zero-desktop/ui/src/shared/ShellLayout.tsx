import { NavLink, Outlet } from 'react-router-dom'
import { useEffect, useState } from 'react'
import { BookOpen, Mic, Cookie, Settings } from 'lucide-react'
import EnvConfigService from '../modules/english/services/EnvConfigService'

const navItems = [
  { to: '/english/annotated', icon: BookOpen, label: '英语听力' },
  { to: '/speech', icon: Mic, label: '语音识别' },
  { to: '/cookie', icon: Cookie, label: 'Cookie 采集' },
  { to: '/settings', icon: Settings, label: '设置' },
]

export default function ShellLayout() {
  return (
    <div className="flex h-screen w-screen overflow-hidden bg-white text-gray-900 dark:bg-gray-950 dark:text-gray-100">
      {/* 左侧导航 */}
      <aside className="flex w-44 flex-shrink-0 flex-col border-r border-gray-200 bg-gray-50 dark:border-gray-800 dark:bg-gray-900">
        <div className="flex h-12 items-center px-4 text-sm font-semibold tracking-wide text-gray-500 dark:text-gray-400">
          zero-desktop
        </div>
        <nav className="flex-1 space-y-1 px-2 py-2">
          {navItems.map(({ to, icon: Icon, label }) => (
            <NavLink
              key={to}
              to={to}
              className={({ isActive }) =>
                [
                  'flex items-center gap-2 rounded-md px-3 py-2 text-sm transition-colors',
                  isActive
                    ? 'bg-blue-100 text-blue-700 dark:bg-blue-900 dark:text-blue-300'
                    : 'text-gray-700 hover:bg-gray-200 dark:text-gray-300 dark:hover:bg-gray-800',
                ].join(' ')
              }
            >
              <Icon size={16} />
              {label}
            </NavLink>
          ))}
        </nav>
      </aside>

      {/* 右侧主区域 */}
      <div className="flex flex-1 flex-col overflow-hidden">
        {/* 顶部状态栏 */}
        <header className="flex h-10 items-center justify-end gap-3 border-b border-gray-200 px-4 text-xs text-gray-500 dark:border-gray-800 dark:text-gray-400">
          <G10StatusIndicator />
          <CustomerIdIndicator />
        </header>

        {/* 内容区 */}
        <main className="flex-1 overflow-auto p-6">
          <Outlet />
        </main>
      </div>
    </div>
  )
}

/** G10 连接状态指示灯（阶段 1 静态灰点，阶段 5 接入真实健康检查）。 */
function G10StatusIndicator() {
  return (
    <span className="flex items-center gap-1.5">
      <span
        className="inline-block h-2 w-2 rounded-full bg-gray-400"
        title="G10 连接状态（未检测）"
      />
      <span>G10</span>
    </span>
  )
}

/** customer_id 配置状态指示灯（绿=已配置，灰=未配置）。 */
function CustomerIdIndicator() {
  const [configured, setConfigured] = useState<boolean | null>(null)

  useEffect(() => {
    EnvConfigService.getInstance().getCustomerId()
      .then(cid => setConfigured(!!cid))
      .catch(() => setConfigured(false))
  }, [])

  return (
    <span className="flex items-center gap-1.5">
      <span
        className={[
          'inline-block h-2 w-2 rounded-full',
          configured === null ? 'bg-gray-400' : configured ? 'bg-green-500' : 'bg-gray-400',
        ].join(' ')}
        title={configured ? 'customer_id 已配置' : 'customer_id 未配置，请到设置页配置'}
      />
      <span>CID</span>
    </span>
  )
}
