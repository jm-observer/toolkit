import { useCallback, useEffect, useRef, useState } from 'react'
import { RefreshCw, Rocket, CircleDot, ExternalLink } from 'lucide-react'
import {
  G10DeployAPI,
  openUrl,
  onDeployDone,
  onDeployLog,
  type DeployLog,
  type LocalVersion,
  type ProbeResult,
  type ServiceDef,
} from './api/tauri-client'

// ── 单服务的合并视图状态 ──────────────────────────────────────────────────────

interface Row {
  def: ServiceDef
  probe?: ProbeResult
  local?: LocalVersion
  probing: boolean
}

// 漂移判定：优先用 commit 对比——远端编译版（health.commit）与本地编译版（git_hash）都有
// 且不同 → 运行版与本地编译漂移。其次 dirty 时提示本地有未提交改动。
function driftHint(probe?: ProbeResult, local?: LocalVersion): string {
  const remoteCommit = probe?.remote_commit ?? null
  const localHash = local?.git_hash ?? null
  if (remoteCommit && localHash && remoteCommit !== localHash) {
    return '运行版与本地编译有漂移'
  }
  if (local?.dirty) return '本地有未提交改动'
  return ''
}

function StatusDot({ probe, configured }: { probe?: ProbeResult; configured: boolean }) {
  let cls = 'text-gray-400'
  let title = '未探测'
  if (!configured) {
    cls = 'text-gray-300'
    title = '未配置健康端点'
  } else if (probe) {
    if (probe.reachable) {
      cls = 'text-green-500'
      title = `在线 ${probe.latency_ms ?? ''}ms`
    } else {
      cls = 'text-red-500'
      title = probe.error ?? '不可达'
    }
  }
  return <CircleDot size={14} className={cls} aria-label={title} />
}

export default function G10DeployPage() {
  const [rows, setRows] = useState<Row[]>([])
  const [warning, setWarning] = useState<string | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)

  // 部署日志面板
  const [deployingName, setDeployingName] = useState<string | null>(null)
  const [logLines, setLogLines] = useState<DeployLog[]>([])
  const [doneMsg, setDoneMsg] = useState<string | null>(null)
  const logEndRef = useRef<HTMLDivElement | null>(null)

  // ── 加载清单 + 逐个探测/取本地版本 ────────────────────────────────────────
  const refreshOne = useCallback(async (name: string) => {
    setRows(prev => prev.map(r => (r.def.name === name ? { ...r, probing: true } : r)))
    const [probe, local] = await Promise.all([
      G10DeployAPI.probe(name).catch(e => ({
        name, reachable: false, status: null, remote_version: null,
        latency_ms: null, error: String(e),
      } as ProbeResult)),
      G10DeployAPI.localVersion(name).catch(e => ({
        name, git_hash: null, dirty: false, error: String(e),
      } as LocalVersion)),
    ])
    setRows(prev =>
      prev.map(r => (r.def.name === name ? { ...r, probe, local, probing: false } : r)),
    )
  }, [])

  const loadAll = useCallback(async () => {
    setLoadError(null)
    try {
      const list = await G10DeployAPI.listServices()
      setWarning(list.warning)
      setRows(list.services.map(def => ({ def, probing: true })))
      // 并发探测每个服务
      await Promise.all(list.services.map(s => refreshOne(s.name)))
    } catch (e) {
      setLoadError(String(e))
    }
  }, [refreshOne])

  useEffect(() => {
    void loadAll()
  }, [loadAll])

  // ── 订阅部署事件 ──────────────────────────────────────────────────────────
  useEffect(() => {
    let unlistenLog: (() => void) | null = null
    let unlistenDone: (() => void) | null = null

    onDeployLog(log => {
      setLogLines(prev => [...prev, log])
    }).then(fn => { unlistenLog = fn })

    onDeployDone(done => {
      setDeployingName(null)
      setDoneMsg(
        done.success
          ? `✅ ${done.name} 部署成功`
          : `❌ ${done.name} 部署失败：${done.error ?? '未知错误'}`,
      )
      // 部署完成后刷新该服务的连通性/版本
      void refreshOne(done.name)
    }).then(fn => { unlistenDone = fn })

    return () => {
      unlistenLog?.()
      unlistenDone?.()
    }
  }, [refreshOne])

  // 日志自动滚到底
  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [logLines])

  const startDeploy = async (name: string) => {
    if (deployingName) {
      window.alert('已有部署正在进行，请等待完成')
      return
    }
    if (!window.confirm(`确认部署 ${name} 到 G10？将交叉编译 → scp → 重启服务。`)) return
    setLogLines([])
    setDoneMsg(null)
    setDeployingName(name)
    try {
      await G10DeployAPI.deploy(name)
    } catch (e) {
      setDeployingName(null)
      window.alert(`部署启动失败：${String(e)}`)
    }
  }

  return (
    <div className="mx-auto max-w-4xl space-y-4">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-semibold">G10 部署管理</h1>
          <p className="text-sm text-gray-500 dark:text-gray-400">
            D:\git 下部署到 G10 的服务：连通性 · 本地编译版 vs 远端运行版 · 一键交叉编译部署
          </p>
        </div>
        <button
          type="button"
          onClick={() => void loadAll()}
          className="flex items-center gap-1.5 rounded-md bg-gray-100 px-3 py-1.5 text-sm hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700"
        >
          <RefreshCw size={14} /> 刷新全部
        </button>
      </header>

      {loadError && (
        <div className="rounded-md bg-red-50 px-3 py-2 text-sm text-red-700 dark:bg-red-950 dark:text-red-300">
          加载清单失败：{loadError}
        </div>
      )}
      {warning && (
        <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-700 dark:bg-amber-950 dark:text-amber-300">
          {warning}
        </div>
      )}

      {/* 服务卡片列表 */}
      <div className="space-y-3">
        {rows.map(({ def, probe, local, probing }) => {
          const configured = def.health_url.length > 0
          const canDeploy = def.deploy != null
          const hint = driftHint(probe, local)
          return (
            <div
              key={def.name}
              className="rounded-lg border border-gray-200 p-4 dark:border-gray-800"
            >
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <StatusDot probe={probe} configured={configured} />
                    <span className="font-medium">{def.label}</span>
                    {def.remote_service && (
                      <span className="rounded bg-gray-100 px-1.5 py-0.5 text-xs text-gray-500 dark:bg-gray-800">
                        {def.remote_service}
                      </span>
                    )}
                  </div>
                  <p className="mt-0.5 truncate text-xs text-gray-500 dark:text-gray-400">
                    {def.note}
                  </p>

                  {/* 版本对比：远端运行版(semver) · 远端编译版(commit) · 本地编译版(commit) */}
                  <div className="mt-2 flex flex-wrap gap-x-5 gap-y-1 text-xs">
                    <span className="text-gray-500">
                      远端运行版：
                      <span className="font-mono text-gray-800 dark:text-gray-200">
                        {probe?.remote_version ?? (probing ? '…' : '—')}
                      </span>
                    </span>
                    <span className="text-gray-500">
                      远端编译版：
                      <span className="font-mono text-gray-800 dark:text-gray-200">
                        {probe?.remote_commit ?? (probing ? '…' : '—')}
                      </span>
                    </span>
                    <span className="text-gray-500">
                      本地编译版：
                      <span className="font-mono text-gray-800 dark:text-gray-200">
                        {local?.git_hash ?? (probing ? '…' : '—')}
                        {local?.dirty ? '*' : ''}
                      </span>
                    </span>
                    {hint && <span className="text-amber-600 dark:text-amber-400">{hint}</span>}
                    {probe?.error && !probe.reachable && (
                      <span className="text-red-500">{probe.error}</span>
                    )}
                  </div>
                </div>

                {/* 操作 */}
                <div className="flex flex-shrink-0 items-center gap-2">
                  {def.web_url && (
                    <button
                      type="button"
                      onClick={() => void openUrl(def.web_url)}
                      title={`打开后台 ${def.web_url}`}
                      className="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-sm text-gray-600 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-gray-800"
                    >
                      <ExternalLink size={14} /> 打开后台
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => void refreshOne(def.name)}
                    title="刷新连通性/版本"
                    className="rounded-md p-1.5 text-gray-500 hover:bg-gray-100 dark:hover:bg-gray-800"
                  >
                    <RefreshCw size={14} className={probing ? 'animate-spin' : ''} />
                  </button>
                  <button
                    type="button"
                    disabled={!canDeploy || deployingName != null}
                    onClick={() => void startDeploy(def.name)}
                    title={canDeploy ? '交叉编译并部署到 G10' : '该服务暂未接入一键部署（脚本待接入）'}
                    className={[
                      'flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm font-medium transition-colors',
                      canDeploy && deployingName == null
                        ? 'bg-blue-500 text-white hover:bg-blue-600'
                        : 'cursor-not-allowed bg-gray-100 text-gray-400 dark:bg-gray-800',
                    ].join(' ')}
                  >
                    <Rocket size={14} />
                    {deployingName === def.name ? '部署中…' : '部署'}
                  </button>
                </div>
              </div>
            </div>
          )
        })}
      </div>

      {/* 部署日志面板 */}
      {(deployingName || logLines.length > 0 || doneMsg) && (
        <div className="rounded-lg border border-gray-200 dark:border-gray-800">
          <div className="flex items-center justify-between border-b border-gray-200 px-3 py-2 text-sm dark:border-gray-800">
            <span className="font-medium">
              部署日志 {deployingName ? `· ${deployingName}（进行中）` : ''}
            </span>
            {doneMsg && <span>{doneMsg}</span>}
          </div>
          <div className="max-h-72 overflow-auto bg-gray-950 p-3 font-mono text-xs leading-relaxed text-gray-200">
            {logLines.map((l, i) => (
              <div key={i} className={l.stream === 'stderr' ? 'text-red-400' : ''}>
                {l.line}
              </div>
            ))}
            <div ref={logEndRef} />
          </div>
        </div>
      )}
    </div>
  )
}
