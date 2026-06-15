import { invoke } from '@tauri-apps/api/core'

// ── 与后端 net_policy 命令对齐的类型 ───────────────────────────────────────────

export type Route = 'direct' | 'wg'
export type RuleKind = 'process-path' | 'process-name' | 'domain-suffix' | 'ip-cidr'

export interface Rule {
  kind: RuleKind
  value: string
  route: Route
}

export interface RuleSet {
  rules: Rule[]
  groups: unknown[]
}

export interface WgConfig {
  server: string
  port: number
  ip: string
  private_key: string
  public_key: string
  pre_shared_key: string
  mtu: number
}

export interface Settings {
  wg: WgConfig
  dns_bootstrap: string[]
  lan_ranges: string[]
  killswitch_enabled: boolean
  block_ipv6: boolean
}

export interface FirewallStatus {
  default_outbound: string
  rule_count: number
  active: boolean
}

export interface Status {
  platform_supported: boolean
  wg_configured: boolean
  killswitch_enabled: boolean
  applied: boolean
  mihomo_running: boolean
  tun_ready: boolean
  protected: boolean
  protection_validated: boolean
  firewall: FirewallStatus | null
}

export interface ProcessCandidate {
  pid: number
  name: string
  path: string
  remotes: string[]
}

export interface VerifyCase {
  id: string
  name: string
  status: string
  observed: string
}

export interface VerifyReport {
  mihomo_running: boolean
  cases: VerifyCase[]
}

// ── 活跃连接快照（P0-1，net_policy_connections） ───────────────────────────────

export interface Connection {
  chains: string[]
  outbound: string
  host: string
  destination_ip: string
  destination_port: string
  process: string
  rule: string
  network: string
}

export interface ConnectionsSnapshot {
  available: boolean
  total: number
  wg_count: number
  direct_count: number
  other_count: number
  by_process: Record<string, number>
  connections: Connection[]
}

// ── apply 进度事件（Phase 2，listen('net-policy://apply-progress')） ────────────

export const APPLY_PROGRESS_EVENT = 'net-policy://apply-progress'

export interface ApplyProgress {
  step: number
  name: string
  status: 'running' | 'ok' | 'fail'
  detail: string | null
}

/** apply 的 6 个阶段（与后端 APPLY_STEPS 对齐，索引从 0 起）。 */
export const APPLY_STEPS = [
  '校验配置',
  '装防火墙基线',
  '启动引擎',
  '等待 TUN 起栈',
  '补 TUN 白名单',
  '验证连通',
]

// 所有命令以 net_policy_ 前缀，集中包装（仿 SpeechAPI）。
export const NetPolicyAPI = {
  getStatus: () => invoke<Status>('net_policy_get_status'),
  getConnections: () => invoke<ConnectionsSnapshot>('net_policy_connections'),
  getSettings: () => invoke<Settings>('net_policy_get_settings'),
  saveSettings: (settings: Settings) => invoke('net_policy_save_settings', { settings }),
  parseWgConf: (content: string) => invoke<WgConfig>('net_policy_parse_wg_conf', { content }),
  listRules: () => invoke<RuleSet>('net_policy_list_rules'),
  saveRule: (rule: Rule) => invoke<RuleSet>('net_policy_save_rule', { rule }),
  deleteRule: (index: number) => invoke<RuleSet>('net_policy_delete_rule', { index }),
  listProcessCandidates: () => invoke<ProcessCandidate[]>('net_policy_list_process_candidates'),
  apply: () => invoke<Status>('net_policy_apply'),
  emergencyStop: () => invoke<Status>('net_policy_emergency_stop'),
  verify: () => invoke<VerifyReport>('net_policy_verify'),
}
