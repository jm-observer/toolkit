import { useEffect, useRef, useState } from 'react'
import type { UnlistenFn } from '@tauri-apps/api/event'
import {
  CodeloopAPI,
  onProgress,
  type Progress,
  type Provider,
  type ReviewMode,
  type SessionMessage,
  type SessionSummary,
} from './api/tauri-client'
import { SessionPairPicker } from './components/SessionPairPicker'
import { MessageColumn } from './components/MessageColumn'
import { LoopStatusBar } from './components/LoopStatusBar'
import { AskUserModal } from './components/AskUserModal'
import { ConfirmGateModal } from './components/ConfirmGateModal'

const POLL_MS = 1500

export default function CodeloopPage() {
  const [sessions, setSessions] = useState<SessionSummary[]>([])
  const [loadingSessions, setLoadingSessions] = useState(false)
  const [sessionsErr, setSessionsErr] = useState<string | null>(null)

  const [claudeId, setClaudeId] = useState('')
  const [codexId, setCodexId] = useState('')
  const [creatingCodex, setCreatingCodex] = useState(false)

  // 双栏消息：cursor 用 ref（不触发渲染），messages 用 state。
  const cursors = useRef<Record<Provider, number>>({ codex: 0, claude: 0 })
  const [messages, setMessages] = useState<Record<Provider, SessionMessage[]>>({ codex: [], claude: [] })

  // 表单
  const [targetPath, setTargetPath] = useState('')
  const [mode, setMode] = useState<ReviewMode>('design')
  const [maxRounds, setMaxRounds] = useState(5)
  const [waitIdle, setWaitIdle] = useState(false)
  const [stepConfirm, setStepConfirm] = useState(true)

  // 循环
  const [running, setRunning] = useState(false)
  const [progress, setProgress] = useState<Progress | null>(null)
  const [startErr, setStartErr] = useState<string | null>(null)
  const [answeredSeq, setAnsweredSeq] = useState(0)
  const [decidedSeq, setDecidedSeq] = useState(0)

  // ── 会话清单 ──────────────────────────────────────────────────────────────
  const refreshSessions = () => {
    setLoadingSessions(true)
    setSessionsErr(null)
    CodeloopAPI.listSessions(30)
      .then(setSessions)
      .catch(e => setSessionsErr(String(e)))
      .finally(() => setLoadingSessions(false))
  }
  useEffect(refreshSessions, [])

  // 内联：id → 项目名（cwd 末段名）。空 cwd / 未选 → 空串。用于同项目联动。
  const projectOf = (id: string): string => {
    if (!id) return ''
    const cwd = sessions.find(s => s.id === id)?.cwd || ''
    const parts = cwd.split(/[/\\]+/).filter(Boolean)
    return parts.length ? parts[parts.length - 1] : ''
  }
  const claudeProject = projectOf(claudeId)
  const codexProject = projectOf(codexId)
  // 两侧都选了但不在同一项目时弱提示（不拦截，启动仍由后端三方校验兜底）。
  const projectMismatch =
    !!claudeProject && !!codexProject && claudeProject !== codexProject

  const onPick = (provider: Provider, id: string) => {
    cursors.current[provider] = 0
    setMessages(m => ({ ...m, [provider]: [] }))
    if (provider === 'claude') setClaudeId(id)
    else setCodexId(id)
  }

  // 新建 Codex 会话：复用所选 Claude 会话的仓库目录，建好后刷新清单并自动选中。
  const handleNewCodex = async () => {
    if (!claudeId || creatingCodex) return
    setCreatingCodex(true)
    setSessionsErr(null)
    try {
      const newId = await CodeloopAPI.newCodexSession(claudeId)
      refreshSessions()
      onPick('codex', newId)
    } catch (e) {
      setSessionsErr(String(e))
    } finally {
      setCreatingCodex(false)
    }
  }

  // ── 双栏消息增量轮询 ─────────────────────────────────────────────────────
  useEffect(() => {
    let alive = true
    const pollSide = async (provider: Provider, id: string) => {
      if (!id) return
      try {
        const page = await CodeloopAPI.sessionMessages(provider, id, cursors.current[provider])
        if (!alive) return
        if (page.messages.length) {
          setMessages(m => ({ ...m, [provider]: [...m[provider], ...page.messages] }))
        }
        cursors.current[provider] = page.cursor
      } catch {
        /* 本地读取抖动：静默跳过本轮，不重置 cursor */
      }
    }
    const tick = () => {
      void pollSide('claude', claudeId)
      void pollSide('codex', codexId)
    }
    tick()
    const t = setInterval(tick, POLL_MS)
    return () => {
      alive = false
      clearInterval(t)
    }
  }, [claudeId, codexId])

  // ── 循环进度（event + 初始快照） ─────────────────────────────────────────
  useEffect(() => {
    let un: UnlistenFn | undefined
    onProgress(p => {
      setProgress(p)
      if (p.phase === 'done' || p.phase === 'error') setRunning(false)
    }).then(f => {
      un = f
    })
    CodeloopAPI.status()
      .then(s => {
        setRunning(s.running)
        if (s.progress) setProgress(s.progress)
      })
      .catch(() => {})
    return () => un?.()
  }, [])

  // ── 启动 / 应答 ──────────────────────────────────────────────────────────
  const canStart = !!claudeId && !!codexId && !!targetPath.trim() && !running
  const handleStart = async () => {
    setStartErr(null)
    try {
      await CodeloopAPI.start({
        claude: { session_id: claudeId },
        codex: { session_id: codexId },
        target_path: targetPath.trim(),
        mode,
        max_rounds: maxRounds,
        wait_for_claude_idle: waitIdle,
        step_confirm: stepConfirm,
      })
      setRunning(true)
      setProgress({ phase: 'starting' })
    } catch (e) {
      setStartErr(String(e))
    }
  }
  const handleStop = async () => {
    try {
      await CodeloopAPI.stop()
    } catch {
      /* ignore */
    }
    setRunning(false)
  }
  const handleAnswer = async (text: string) => {
    const seq = progress?.seq
    if (seq == null) return
    try {
      await CodeloopAPI.answer(seq, text)
      setAnsweredSeq(seq)
    } catch (e) {
      setStartErr(String(e))
    }
  }

  const handleDecide = async (approve: boolean) => {
    const seq = progress?.seq
    if (seq == null) return
    setDecidedSeq(seq) // 乐观关窗，避免重复点击
    try {
      await CodeloopAPI.confirm(seq, approve)
    } catch (e) {
      setStartErr(String(e))
    }
  }

  // ASK_USER 弹窗：进入 awaiting_input 且该 seq 未答过。
  const showAsk =
    progress?.phase === 'awaiting_input' &&
    progress.seq != null &&
    progress.seq > answeredSeq &&
    !!progress.question

  // 逐步确认弹窗：运行中、进入 awaiting_confirm 且该 seq 未拍板过。
  const showConfirm =
    running &&
    progress?.phase === 'awaiting_confirm' &&
    progress.seq != null &&
    progress.seq > decidedSeq

  return (
    <div className="flex h-full flex-col gap-3">
      <div>
        <h1 className="text-xl font-semibold">复核循环</h1>
        <p className="mt-1 text-xs text-gray-500 dark:text-gray-400">
          关联一对 Codex / Claude Code 会话，驱动「复核 ↔ 修订」往复。默认逐步确认：每次跨会话传递前弹窗等你拍板（本机直跑，无需额外进程）。
        </p>
      </div>

      <SessionPairPicker
        sessions={sessions}
        claudeId={claudeId}
        codexId={codexId}
        onPick={onPick}
        onRefresh={refreshSessions}
        loading={loadingSessions}
        onNewCodex={handleNewCodex}
        creatingCodex={creatingCodex}
        claudeProject={claudeProject}
        codexProject={codexProject}
      />
      {projectMismatch && (
        <div className="text-xs text-amber-600 dark:text-amber-400">
          Claude（{claudeProject}）与 Codex（{codexProject}）不在同一项目，启动时会校验失败。
        </div>
      )}
      {sessionsErr && (
        <div className="rounded-md bg-red-50 px-3 py-2 text-xs text-red-600 dark:bg-red-900/20 dark:text-red-400">
          会话清单加载失败：{sessionsErr}（确认本机 codex / claude 已产生过会话）
        </div>
      )}

      <LoopStatusBar
        targetPath={targetPath}
        setTargetPath={setTargetPath}
        mode={mode}
        setMode={setMode}
        maxRounds={maxRounds}
        setMaxRounds={setMaxRounds}
        waitIdle={waitIdle}
        setWaitIdle={setWaitIdle}
        stepConfirm={stepConfirm}
        setStepConfirm={setStepConfirm}
        running={running}
        canStart={canStart}
        onStart={handleStart}
        onStop={handleStop}
        progress={progress}
      />
      {startErr && (
        <div className="rounded-md bg-red-50 px-3 py-2 text-xs text-red-600 dark:bg-red-900/20 dark:text-red-400">
          {startErr}
        </div>
      )}

      <div className="flex min-h-0 flex-1 gap-3">
        <MessageColumn title="Claude Code" sessionId={claudeId} messages={messages.claude} />
        <MessageColumn title="Codex" sessionId={codexId} messages={messages.codex} />
      </div>

      {showAsk && progress?.question && (
        <AskUserModal
          question={progress.question}
          seq={progress.seq!}
          askedBy={progress.asked_by}
          onAnswer={handleAnswer}
        />
      )}

      {showConfirm && (
        <ConfirmGateModal
          seq={progress!.seq!}
          direction={progress!.direction}
          title={progress!.title}
          content={progress!.content}
          onApprove={() => handleDecide(true)}
          onReject={() => handleDecide(false)}
        />
      )}
    </div>
  )
}
