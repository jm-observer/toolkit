import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { ShieldCheck, RefreshCw, Plus, Trash2, Play, OctagonX, FlaskConical, Upload } from 'lucide-react'

// ── 与后端 net_policy 命令对齐的类型 ───────────────────────────────────────────

type Route = 'direct' | 'wg'
type RuleKind = 'process-path' | 'process-name' | 'domain-suffix' | 'ip-cidr'

interface Rule {
  kind: RuleKind
  value: string
  route: Route
}

interface RuleSet {
  rules: Rule[]
  groups: unknown[]
}

interface WgConfig {
  server: string
  port: number
  ip: string
  private_key: string
  public_key: string
  pre_shared_key: string
  mtu: number
}

interface Settings {
  wg: WgConfig
  dns_bootstrap: string[]
  lan_ranges: string[]
  killswitch_enabled: boolean
  block_ipv6: boolean
}

interface FirewallStatus {
  default_outbound: string
  rule_count: number
  active: boolean
}

interface Status {
  platform_supported: boolean
  wg_configured: boolean
  killswitch_enabled: boolean
  applied: boolean
  mihomo_running: boolean
  protected: boolean
  firewall: FirewallStatus | null
}

interface ProcessCandidate {
  pid: number
  name: string
  path: string
  remotes: string[]
}

interface VerifyCase {
  id: string
  name: string
  status: string
  observed: string
}
interface VerifyReport {
  mihomo_running: boolean
  cases: VerifyCase[]
}

const KIND_LABELS: Record<RuleKind, string> = {
  'process-path': '程序路径',
  'process-name': '程序名',
  'domain-suffix': '域名后缀',
  'ip-cidr': 'IP/CIDR',
}

// ── 小组件 ────────────────────────────────────────────────────────────────────

function Panel({ title, children, right }: { title: string; children: React.ReactNode; right?: React.ReactNode }) {
  return (
    <section className="rounded-lg border border-gray-200 dark:border-gray-800">
      <div className="flex items-center justify-between border-b border-gray-200 px-4 py-2 dark:border-gray-800">
        <h2 className="text-sm font-semibold">{title}</h2>
        {right}
      </div>
      <div className="p-4">{children}</div>
    </section>
  )
}

function Light({ on, label }: { on: boolean | 'warn'; label: string }) {
  const cls = on === 'warn' ? 'bg-yellow-400' : on ? 'bg-green-500' : 'bg-gray-400'
  return (
    <span className="flex items-center gap-1.5 text-xs">
      <span className={`inline-block h-2 w-2 rounded-full ${cls}`} />
      {label}
    </span>
  )
}

function btn(variant: 'primary' | 'danger' | 'ghost' = 'ghost') {
  const base = 'inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm transition-colors disabled:opacity-50'
  if (variant === 'primary') return `${base} bg-blue-600 text-white hover:bg-blue-700`
  if (variant === 'danger') return `${base} bg-red-600 text-white hover:bg-red-700`
  return `${base} border border-gray-300 hover:bg-gray-100 dark:border-gray-700 dark:hover:bg-gray-800`
}

// ── 主页面 ────────────────────────────────────────────────────────────────────

export default function NetPolicyPage() {
  const [status, setStatus] = useState<Status | null>(null)
  const [settings, setSettings] = useState<Settings | null>(null)
  const [rules, setRules] = useState<RuleSet>({ rules: [], groups: [] })
  const [candidates, setCandidates] = useState<ProcessCandidate[]>([])
  const [verify, setVerify] = useState<VerifyReport | null>(null)
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<{ kind: 'ok' | 'err'; text: string } | null>(null)

  // 新规则表单
  const [newRule, setNewRule] = useState<Rule>({ kind: 'process-name', value: '', route: 'direct' })

  // 导入 WireGuard .conf 用的隐藏 file input
  const wgFileRef = useRef<HTMLInputElement>(null)

  const flash = (kind: 'ok' | 'err', text: string) => {
    setMsg({ kind, text })
    setTimeout(() => setMsg(null), 5000)
  }

  // 读取用户选择的 wg-quick .conf，交后端解析后合并进当前设置（不直接保存，待用户确认）。
  const importWgConf = useCallback(async (file: File) => {
    try {
      const content = await file.text()
      const wg = await invoke<WgConfig>('net_policy_parse_wg_conf', { content })
      setSettings(prev => (prev ? { ...prev, wg } : prev))
      flash('ok', '已导入 WireGuard 配置，请检查各字段后点「保存」')
    } catch (e) {
      flash('err', `导入失败: ${String(e)}`)
    }
  }, [])

  const refresh = useCallback(async () => {
    try {
      const [s, st, r] = await Promise.all([
        invoke<Status>('net_policy_get_status'),
        invoke<Settings>('net_policy_get_settings'),
        invoke<RuleSet>('net_policy_list_rules'),
      ])
      setStatus(s)
      setSettings(st)
      setRules(r)
    } catch (e) {
      flash('err', `加载失败: ${String(e)}`)
    }
  }, [])

  useEffect(() => { void refresh() }, [refresh])

  const run = async (label: string, fn: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await fn()
      flash('ok', `${label}成功`)
    } catch (e) {
      flash('err', `${label}失败: ${String(e)}`)
    } finally {
      setBusy(false)
      void refresh()
    }
  }

  const saveSettings = () => settings && run('保存设置', () => invoke('net_policy_save_settings', { settings }))
  const apply = () => run('应用策略', () => invoke('net_policy_apply'))
  const emergencyStop = () => run('紧急停止', () => invoke('net_policy_emergency_stop'))
  const addRule = () =>
    newRule.value.trim() &&
    run('新增规则', async () => {
      const rs = await invoke<RuleSet>('net_policy_save_rule', { rule: { ...newRule, value: newRule.value.trim() } })
      setRules(rs)
      setNewRule({ ...newRule, value: '' })
    })
  const deleteRule = (index: number) =>
    run('删除规则', async () => setRules(await invoke<RuleSet>('net_policy_delete_rule', { index })))
  const loadCandidates = () =>
    run('扫描进程', async () => setCandidates(await invoke<ProcessCandidate[]>('net_policy_list_process_candidates')))
  const addCandidateDirect = (c: ProcessCandidate) =>
    run('加入直连', async () =>
      setRules(await invoke<RuleSet>('net_policy_save_rule', { rule: { kind: 'process-name', value: c.name, route: 'direct' } })),
    )
  const runVerify = () => run('验证', async () => setVerify(await invoke<VerifyReport>('net_policy_verify')))

  return (
    <div className="mx-auto max-w-4xl space-y-5">
      <div className="flex items-center gap-2">
        <ShieldCheck className="text-blue-600" />
        <h1 className="text-lg font-semibold">网络出口策略</h1>
        <button className={btn() + ' ml-auto'} onClick={() => void refresh()} disabled={busy}>
          <RefreshCw size={14} /> 刷新
        </button>
      </div>

      {msg && (
        <div className={`rounded-md px-4 py-2 text-sm ${msg.kind === 'ok' ? 'bg-green-100 text-green-800' : 'bg-red-100 text-red-800'}`}>
          {msg.text}
        </div>
      )}

      {status && !status.platform_supported && (
        <div className="rounded-md bg-yellow-100 px-4 py-2 text-sm text-yellow-800">
          net-policy 仅支持 Windows，当前平台不可用。
        </div>
      )}

      {/* 状态 + 操作 */}
      <Panel
        title="状态"
        right={
          <div className="flex gap-2">
            <button className={btn('primary')} onClick={apply} disabled={busy || !status?.wg_configured}>
              <Play size={14} /> 应用
            </button>
            <button className={btn('danger')} onClick={emergencyStop} disabled={busy}>
              <OctagonX size={14} /> 紧急停止
            </button>
          </div>
        }
      >
        {status && (
          <div className="space-y-2">
            <div className="flex flex-wrap gap-4">
              <Light on={status.wg_configured} label="WG 已配置" />
              <Light on={status.mihomo_running} label="mihomo 运行" />
              <Light on={status.protected ? true : status.applied ? 'warn' : false} label={status.protected ? '受保护 (fail-closed)' : status.applied ? '不受保护预览' : '未应用'} />
              <Light on={status.killswitch_enabled ? (status.firewall?.active ? true : 'warn') : false} label={`kill-switch${status.firewall ? ` (${status.firewall.default_outbound}/${status.firewall.rule_count}条)` : ''}`} />
            </div>
            {status.applied && !status.protected && (
              <div className="rounded-md bg-amber-100 px-3 py-1.5 text-xs text-amber-800">
                ⚠ 当前为<b>不受保护预览</b>模式：mihomo 在跑但防火墙 kill-switch 未生效。mihomo/TUN/WG 任一异常时未知流量可能泄漏到本地出口。生产请开启 kill-switch。
              </div>
            )}
          </div>
        )}
      </Panel>

      {/* WG 设置 */}
      {settings && (
        <Panel
          title="WireGuard 出口 + 设置"
          right={
            <div className="flex gap-2">
              <button className={btn()} onClick={() => wgFileRef.current?.click()} disabled={busy} title="从 WireGuard .conf 文件导入">
                <Upload size={14} /> 导入配置
              </button>
              <button className={btn('primary')} onClick={saveSettings} disabled={busy}>保存</button>
            </div>
          }
        >
          <input
            ref={wgFileRef}
            type="file"
            accept=".conf,text/plain"
            className="hidden"
            onChange={e => {
              const f = e.target.files?.[0]
              if (f) void importWgConf(f)
              e.target.value = '' // 允许重复选同一文件
            }}
          />
          <div className="grid grid-cols-2 gap-3 text-sm">
            <label className="flex flex-col gap-1">服务端 IP
              <input className="rounded border px-2 py-1 dark:bg-gray-800 dark:border-gray-700" value={settings.wg.server}
                onChange={e => setSettings({ ...settings, wg: { ...settings.wg, server: e.target.value } })} placeholder="38.x.x.x（必须是 IP）" />
            </label>
            <label className="flex flex-col gap-1">端口
              <input type="number" className="rounded border px-2 py-1 dark:bg-gray-800 dark:border-gray-700" value={settings.wg.port}
                onChange={e => setSettings({ ...settings, wg: { ...settings.wg, port: Number(e.target.value) } })} />
            </label>
            <label className="flex flex-col gap-1">隧道内本机 IP
              <input className="rounded border px-2 py-1 dark:bg-gray-800 dark:border-gray-700" value={settings.wg.ip}
                onChange={e => setSettings({ ...settings, wg: { ...settings.wg, ip: e.target.value } })} placeholder="10.66.66.x" />
            </label>
            <label className="flex flex-col gap-1">MTU
              <input type="number" className="rounded border px-2 py-1 dark:bg-gray-800 dark:border-gray-700" value={settings.wg.mtu}
                onChange={e => setSettings({ ...settings, wg: { ...settings.wg, mtu: Number(e.target.value) } })} />
            </label>
            <label className="col-span-2 flex flex-col gap-1">本机私钥
              <input className="rounded border px-2 py-1 font-mono text-xs dark:bg-gray-800 dark:border-gray-700" value={settings.wg.private_key}
                onChange={e => setSettings({ ...settings, wg: { ...settings.wg, private_key: e.target.value } })} />
            </label>
            <label className="col-span-2 flex flex-col gap-1">服务端公钥
              <input className="rounded border px-2 py-1 font-mono text-xs dark:bg-gray-800 dark:border-gray-700" value={settings.wg.public_key}
                onChange={e => setSettings({ ...settings, wg: { ...settings.wg, public_key: e.target.value } })} />
            </label>
            <label className="col-span-2 flex flex-col gap-1">预共享密钥（可选）
              <input className="rounded border px-2 py-1 font-mono text-xs dark:bg-gray-800 dark:border-gray-700" value={settings.wg.pre_shared_key}
                onChange={e => setSettings({ ...settings, wg: { ...settings.wg, pre_shared_key: e.target.value } })} />
            </label>
          </div>
          <div className="mt-3 flex flex-wrap items-center gap-4 text-sm">
            <label className="flex items-center gap-2">
              <input type="checkbox" checked={settings.killswitch_enabled}
                onChange={e => setSettings({ ...settings, killswitch_enabled: e.target.checked })} />
              防火墙 kill-switch（fail-closed，<b>建议保持开启</b>）
            </label>
            <label className="flex items-center gap-2">
              <input type="checkbox" checked={settings.block_ipv6}
                onChange={e => setSettings({ ...settings, block_ipv6: e.target.checked })} />
              阻断 IPv6 公网（kill-switch 生效时）
            </label>
            <span className="text-xs text-gray-500">DNS bootstrap: {settings.dns_bootstrap.join(', ')}</span>
          </div>
          {!settings.killswitch_enabled && (
            <div className="mt-2 rounded-md bg-amber-100 px-3 py-1.5 text-xs text-amber-800">
              ⚠ 关闭 kill-switch = <b>不受保护预览</b>模式，失去 fail-closed 兜底；阻断 IPv6 也不会生效。
            </div>
          )}
        </Panel>
      )}

      {/* 分流规则 */}
      <Panel title="分流规则（命中走本地直连，未命中默认走 WG）">
        <div className="mb-3 flex flex-wrap items-end gap-2 text-sm">
          <select className="rounded border px-2 py-1 dark:bg-gray-800 dark:border-gray-700" value={newRule.kind}
            onChange={e => setNewRule({ ...newRule, kind: e.target.value as RuleKind })}>
            {Object.entries(KIND_LABELS).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
          </select>
          <input className="flex-1 rounded border px-2 py-1 dark:bg-gray-800 dark:border-gray-700" placeholder="值（如 steam.exe / example.cn / 1.2.3.0/24）"
            value={newRule.value} onChange={e => setNewRule({ ...newRule, value: e.target.value })}
            onKeyDown={e => e.key === 'Enter' && addRule()} />
          <select className="rounded border px-2 py-1 dark:bg-gray-800 dark:border-gray-700" value={newRule.route}
            onChange={e => setNewRule({ ...newRule, route: e.target.value as Route })}>
            <option value="direct">本地直连</option>
            <option value="wg">走 WG</option>
          </select>
          <button className={btn('primary')} onClick={addRule} disabled={busy}><Plus size={14} /> 添加</button>
        </div>
        <ul className="divide-y divide-gray-200 text-sm dark:divide-gray-800">
          {rules.rules.length === 0 && <li className="py-2 text-gray-500">暂无规则——未知流量全部走 WG 海外出口。</li>}
          {rules.rules.map((r, i) => (
            <li key={i} className="flex items-center gap-2 py-1.5">
              <span className="w-20 text-gray-500">{KIND_LABELS[r.kind]}</span>
              <span className="flex-1 font-mono text-xs">{r.value}</span>
              <span className={`rounded px-1.5 py-0.5 text-xs ${r.route === 'direct' ? 'bg-amber-100 text-amber-800' : 'bg-blue-100 text-blue-800'}`}>
                {r.route === 'direct' ? '直连' : 'WG'}
              </span>
              <button className="text-gray-400 hover:text-red-600" onClick={() => deleteRule(i)} disabled={busy} title="删除">
                <Trash2 size={14} />
              </button>
            </li>
          ))}
        </ul>
      </Panel>

      {/* 进程发现 */}
      <Panel title="进程发现" right={<button className={btn()} onClick={loadCandidates} disabled={busy}>扫描近期连接</button>}>
        <ul className="divide-y divide-gray-200 text-sm dark:divide-gray-800">
          {candidates.length === 0 && <li className="py-2 text-gray-500">点击「扫描近期连接」列出有公网连接的进程。</li>}
          {candidates.map(c => (
            <li key={c.pid} className="flex items-center gap-2 py-1.5">
              <span className="flex-1 truncate" title={c.path}>{c.name || `pid ${c.pid}`} <span className="text-xs text-gray-400">({c.remotes.length} 连接)</span></span>
              <button className={btn()} onClick={() => addCandidateDirect(c)} disabled={busy}>设为直连</button>
            </li>
          ))}
        </ul>
      </Panel>

      {/* 验证 */}
      <Panel title="验证" right={<button className={btn()} onClick={runVerify} disabled={busy}><FlaskConical size={14} /> 一键验证</button>}>
        {!verify && <p className="text-sm text-gray-500">应用策略后点「一键验证」检查出口 IP / DNS 劫持 / 引擎状态。</p>}
        {verify && (
          <ul className="space-y-1 text-sm">
            {verify.cases.map(c => (
              <li key={c.id} className="flex items-center gap-2">
                <Light on={c.status === 'passed' ? true : c.status === 'failed' ? false : 'warn'} label={c.name} />
                <span className="font-mono text-xs text-gray-500">{c.observed}</span>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </div>
  )
}
