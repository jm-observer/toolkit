import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

// ── 类型（与 src/modules/g10_deploy/{registry,mod}.rs 对齐） ──────────────────

export interface DeployDef {
  script: string
  args: string[]
}

export interface ServiceDef {
  name: string
  label: string
  note: string
  repo_dir: string
  health_url: string
  remote_service: string | null
  deploy: DeployDef | null
}

export interface ServiceList {
  services: ServiceDef[]
  warning: string | null
}

export interface ProbeResult {
  name: string
  reachable: boolean
  status: string | null
  remote_version: string | null
  latency_ms: number | null
  error: string | null
}

export interface LocalVersion {
  name: string
  git_hash: string | null
  dirty: boolean
  error: string | null
}

export interface DeployLog {
  name: string
  stream: 'stdout' | 'stderr'
  line: string
}

export interface DeployDone {
  name: string
  success: boolean
  code: number | null
  error: string | null
}

// ── 命令封装 ────────────────────────────────────────────────────────────────

export const G10DeployAPI = {
  listServices: () => invoke<ServiceList>('g10_list_services'),
  probe: (name: string) => invoke<ProbeResult>('g10_probe_service', { name }),
  localVersion: (name: string) => invoke<LocalVersion>('g10_local_version', { name }),
  isDeploying: () => invoke<boolean>('g10_is_deploying'),
  deploy: (name: string) => invoke<void>('g10_deploy', { name }),
}

// ── 部署事件订阅 ──────────────────────────────────────────────────────────────

export function onDeployLog(cb: (log: DeployLog) => void): Promise<UnlistenFn> {
  return listen<DeployLog>('g10-deploy://log', e => cb(e.payload))
}

export function onDeployDone(cb: (done: DeployDone) => void): Promise<UnlistenFn> {
  return listen<DeployDone>('g10-deploy://done', e => cb(e.payload))
}
