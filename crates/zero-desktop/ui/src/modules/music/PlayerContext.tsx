/**
 * PlayerContext —— 音乐播放全局控制面（无 `<audio>`）。
 *
 * 挂在 App 的 ShellLayout 外层，整个生命周期常驻：切路由不卸载 → 事件订阅与
 * 状态不中断（播放本就在后端，UI 卸载也不停）。
 *
 * 职责：
 *  - 启动时 listen 三个事件（state / progress / track_changed）+ 另外两个
 *    （format_changed / error）→ React state。
 *  - 首屏 music_get_state 拉初值（自愈启动竞态）。
 *  - 暴露 play/pause/resume/toggle/seek/next/prev/setVolume… —— 全是 invoke，
 *    UI 无任何本地播放逻辑。
 *  - 已选目录 / 音量 / repeat / shuffle 用 plugin-store 持久化。
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { Store } from '@tauri-apps/plugin-store'
import {
  musicGetState,
  musicNext,
  musicPause,
  musicPlayQueue,
  musicPrev,
  musicResume,
  musicSeek,
  musicSetRepeat,
  musicSetShuffle,
  musicSetVolume,
  musicStop,
  musicToggle,
  onMusicError,
  onMusicFormatChanged,
  onMusicProgress,
  onMusicStateChanged,
  onMusicTrackChanged,
  type MusicFormatChanged,
  type PlaybackState,
  type PlaybackStatus,
  type RepeatMode,
  type Track,
} from './api/tauri-client'

const STORE_FILE = 'music-player.json'
const KEY_FOLDER = 'folder'
const KEY_VOLUME = 'volume'
const KEY_REPEAT = 'repeat'
const KEY_SHUFFLE = 'shuffle'

interface PlayerContextValue {
  // 状态（事件驱动 + 首屏拉取）
  status: PlaybackStatus
  index: number
  track: Track | null
  positionSecs: number
  durationSecs: number
  volume: number
  repeat: RepeatMode
  shuffle: boolean
  format: MusicFormatChanged | null
  error: string | null

  // 持久化的已选目录（曲库根）
  folder: string | null
  setFolder: (dir: string | null) => void

  // 控制（全 invoke）
  play: (paths: string[], start: number) => Promise<void>
  pause: () => Promise<void>
  resume: () => Promise<void>
  toggle: () => Promise<void>
  stop: () => Promise<void>
  seek: (secs: number) => Promise<void>
  next: () => Promise<void>
  prev: () => Promise<void>
  setVolume: (vol: number) => Promise<void>
  setRepeat: (mode: RepeatMode) => Promise<void>
  setShuffle: (on: boolean) => Promise<void>
}

const PlayerContext = createContext<PlayerContextValue | null>(null)

export function usePlayer(): PlayerContextValue {
  const ctx = useContext(PlayerContext)
  if (!ctx) throw new Error('usePlayer 必须在 <MusicPlayerProvider> 内使用')
  return ctx
}

export function MusicPlayerProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<PlaybackStatus>('stopped')
  const [index, setIndex] = useState(0)
  const [track, setTrack] = useState<Track | null>(null)
  const [positionSecs, setPositionSecs] = useState(0)
  const [durationSecs, setDurationSecs] = useState(0)
  const [volume, setVolumeState] = useState(1)
  const [repeat, setRepeatState] = useState<RepeatMode>('off')
  const [shuffle, setShuffleState] = useState(false)
  const [format, setFormat] = useState<MusicFormatChanged | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [folder, setFolderState] = useState<string | null>(null)

  const storeRef = useRef<Store | null>(null)

  // 应用后端权威快照到本地 state。
  const applyState = useCallback((s: PlaybackState) => {
    setStatus(s.status)
    setIndex(s.index)
    setTrack(s.track)
    setPositionSecs(s.position_secs)
    setDurationSecs(s.duration_secs)
    setVolumeState(s.volume)
    setRepeatState(s.repeat)
    setShuffleState(s.shuffle)
  }, [])

  // ── 持久化加载 + 事件订阅 + 首屏拉取 ───────────────────────────────────────
  useEffect(() => {
    let mounted = true
    const unlistens: Array<() => void> = []

    async function init() {
      // 1) plugin-store 读已选目录 / 音量 / repeat / shuffle
      try {
        const store = await Store.load(STORE_FILE)
        storeRef.current = store
        const f = await store.get<string>(KEY_FOLDER)
        const v = await store.get<number>(KEY_VOLUME)
        const r = await store.get<RepeatMode>(KEY_REPEAT)
        const sh = await store.get<boolean>(KEY_SHUFFLE)
        if (!mounted) return
        if (f) setFolderState(f)
        if (typeof v === 'number') setVolumeState(v)
        if (r === 'off' || r === 'one' || r === 'all') setRepeatState(r)
        if (typeof sh === 'boolean') setShuffleState(sh)
      } catch (e) {
        console.error('[MusicPlayer] store 加载失败:', e)
      }

      // 2) 订阅事件
      try {
        unlistens.push(
          await onMusicStateChanged(p => {
            setStatus(p.status)
            setIndex(p.index)
            setTrack(p.track)
          }),
        )
        unlistens.push(
          await onMusicProgress(p => {
            setPositionSecs(p.position_secs)
            setDurationSecs(p.duration_secs)
          }),
        )
        unlistens.push(
          await onMusicTrackChanged(p => {
            setIndex(p.index)
            setTrack(p.track)
          }),
        )
        unlistens.push(
          await onMusicFormatChanged(p => {
            setFormat(p)
          }),
        )
        unlistens.push(
          await onMusicError(p => {
            setError(p.message)
          }),
        )
      } catch (e) {
        console.error('[MusicPlayer] 事件订阅失败:', e)
      }

      // 3) 首屏拉初值（自愈启动竞态）
      try {
        const s = await musicGetState()
        if (mounted) applyState(s)
      } catch {
        /* 后端尚未就绪则忽略，后续事件会补 */
      }
    }

    void init()

    // 周期兜底拉取后端真值，自愈漏接事件。
    const poll = setInterval(() => {
      musicGetState()
        .then(s => {
          if (mounted) applyState(s)
        })
        .catch(() => {/* 忽略抖动 */})
    }, 3000)

    return () => {
      mounted = false
      clearInterval(poll)
      unlistens.forEach(fn => fn())
    }
  }, [applyState])

  const persist = useCallback(async (key: string, value: unknown) => {
    const store = storeRef.current
    if (!store) return
    try {
      await store.set(key, value)
      await store.save()
    } catch (e) {
      console.error('[MusicPlayer] 持久化失败:', key, e)
    }
  }, [])

  // ── 已选目录 ───────────────────────────────────────────────────────────────
  const setFolder = useCallback(
    (dir: string | null) => {
      setFolderState(dir)
      void persist(KEY_FOLDER, dir)
    },
    [persist],
  )

  // ── 控制方法（全 invoke，乐观更新本地控件态再以后端事件为准） ───────────────
  const play = useCallback((paths: string[], start: number) => musicPlayQueue(paths, start), [])
  const pause = useCallback(() => musicPause(), [])
  const resume = useCallback(() => musicResume(), [])
  const toggle = useCallback(() => musicToggle(), [])
  const stop = useCallback(() => musicStop(), [])
  const seek = useCallback((secs: number) => {
    setPositionSecs(secs) // 即时反馈，随后端 progress 事件校正
    return musicSeek(secs)
  }, [])
  const next = useCallback(() => musicNext(), [])
  const prev = useCallback(() => musicPrev(), [])

  const setVolume = useCallback(
    (vol: number) => {
      setVolumeState(vol)
      void persist(KEY_VOLUME, vol)
      return musicSetVolume(vol)
    },
    [persist],
  )

  const setRepeat = useCallback(
    (mode: RepeatMode) => {
      setRepeatState(mode)
      void persist(KEY_REPEAT, mode)
      return musicSetRepeat(mode)
    },
    [persist],
  )

  const setShuffle = useCallback(
    (on: boolean) => {
      setShuffleState(on)
      void persist(KEY_SHUFFLE, on)
      return musicSetShuffle(on)
    },
    [persist],
  )

  const value = useMemo<PlayerContextValue>(
    () => ({
      status,
      index,
      track,
      positionSecs,
      durationSecs,
      volume,
      repeat,
      shuffle,
      format,
      error,
      folder,
      setFolder,
      play,
      pause,
      resume,
      toggle,
      stop,
      seek,
      next,
      prev,
      setVolume,
      setRepeat,
      setShuffle,
    }),
    [
      status,
      index,
      track,
      positionSecs,
      durationSecs,
      volume,
      repeat,
      shuffle,
      format,
      error,
      folder,
      setFolder,
      play,
      pause,
      resume,
      toggle,
      stop,
      seek,
      next,
      prev,
      setVolume,
      setRepeat,
      setShuffle,
    ],
  )

  return <PlayerContext.Provider value={value}>{children}</PlayerContext.Provider>
}
