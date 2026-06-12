/**
 * AnnotationPlayer — 标注/全量句子播放器（Tailwind 重写，无 AntD）。
 */

import { useState, useEffect, useRef } from 'react'
import { Loader2, AlertCircle, RefreshCw, Music } from 'lucide-react'
import AudioPlayer from './AudioPlayer'
import { AudioPlayerService } from '../services/AudioPlayerService'
import ApiService from '../services/ApiService'
import FileCacheManager from '../services/FileCacheManager'
import EnvConfigService from '../services/EnvConfigService'
import HtmlAudioAdapter from '../adapters/HtmlAudioAdapter'
import type { Sentence } from '../types'
import { Button } from '../../speech/components/ui/Button'

interface AnnotationPlayerProps {
  autoStart?: boolean
  dataSource?: 'annotated' | 'all'
}

// 跨 React Strict Mode 挂载周期的全局初始化状态
const componentInstanceState = {
  lastDataSource: null as string | null,
  lastInitTime: 0,
  isInitializing: false
}

export default function AnnotationPlayer({ autoStart = true, dataSource = 'annotated' }: AnnotationPlayerProps) {
  const [loading, setLoading] = useState(false)
  const [loadingText, setLoadingText] = useState(dataSource === 'annotated' ? '正在加载标注句子...' : '正在加载所有句子...')
  const [error, setError] = useState<string | null>(null)
  const [sentences, setSentences] = useState<Sentence[]>([])
  const [initialized, setInitialized] = useState(false)
  const initStartedRef = useRef(false)
  const autoPlayStartedRef = useRef(false)
  const backgroundDownloadCancelRef = useRef(false)
  const cleanupExecutedRef = useRef(false)

  useEffect(() => {
    const now = Date.now()

    if (componentInstanceState.lastDataSource === dataSource && componentInstanceState.isInitializing) return
    if (componentInstanceState.lastDataSource === dataSource && now - componentInstanceState.lastInitTime < 500) return

    componentInstanceState.isInitializing = true
    componentInstanceState.lastDataSource = dataSource
    componentInstanceState.lastInitTime = now

    // 切换 dataSource 时先停止播放
    try { AudioPlayerService.getInstance().stopAudio() } catch { /* not yet initialized */ }

    backgroundDownloadCancelRef.current = true
    initStartedRef.current = false
    autoPlayStartedRef.current = false
    cleanupExecutedRef.current = false
    setInitialized(false)
    setSentences([])
    setError(null)
    backgroundDownloadCancelRef.current = false

    void init().finally(() => { componentInstanceState.isInitializing = false })

    return () => {
      if (cleanupExecutedRef.current) return
      cleanupExecutedRef.current = true
      backgroundDownloadCancelRef.current = true
      try { AudioPlayerService.getInstance().stopAudio() } catch { /* ignore */ }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dataSource])

  useEffect(() => {
    cleanupExecutedRef.current = false
    return () => {
      if (cleanupExecutedRef.current) return
      cleanupExecutedRef.current = true
      try { AudioPlayerService.getInstance().stopAudio() } catch { /* ignore */ }
      backgroundDownloadCancelRef.current = true
    }
  }, [])

  const init = async () => {
    if (initStartedRef.current) return
    initStartedRef.current = true

    try {
      setLoading(true)
      setLoadingText(dataSource === 'annotated' ? '正在获取标注句子列表...' : '正在获取所有句子列表...')
      setError(null)

      const sentencesList = await loadSentences()
      await cacheAudioFiles(sentencesList)

      setLoadingText('正在初始化播放器...')
      await new Promise(resolve => setTimeout(resolve, 100))
      await initAudioPlayer(sentencesList)

      setLoading(false)
      setInitialized(true)
    } catch (err: any) {
      console.error('播放器初始化失败:', err)
      setLoading(false)
      setError(err.message || '初始化失败')
    }
  }

  const loadSentences = async (): Promise<Sentence[]> => {
    setLoadingText(dataSource === 'annotated' ? '正在获取标注句子列表...' : '正在获取所有句子列表...')
    const apiService = ApiService.getInstance()
    const response = dataSource === 'annotated'
      ? await apiService.getAnnotatedSentences()
      : await apiService.getAllSentences()

    const list: Sentence[] = response.data || []
    setSentences(list)

    if (list.length === 0) {
      throw new Error(dataSource === 'annotated' ? '没有标注句子' : '没有句子')
    }
    return list
  }

  const cacheAudioFiles = async (sentencesList: Sentence[]) => {
    if (!sentencesList.length) return

    // EnvConfig 用于缓存 manager（apiBaseUrl 备用）
    const envConfig = await EnvConfigService.getInstance().getConfig()
    const cacheManager = FileCacheManager.getInstance()

    const isAllMode = dataSource === 'all'
    const primary = isAllMode ? sentencesList.slice(0, 50) : sentencesList
    const remaining = isAllMode ? sentencesList.slice(50) : []

    let cachedCount = 0
    const totalCount = primary.reduce((acc, s) => acc + s.audios.length, 0)

    for (const sentence of primary) {
      for (const audio of sentence.audios) {
        setLoadingText(`正在缓存音频... (${cachedCount + 1}/${totalCount})`)
        try {
          await cacheManager.downloadAndCache(audio.id, envConfig.apiBaseUrl)
          cachedCount++
        } catch (err) {
          console.error(`缓存音频失败 (ID: ${audio.id}):`, err)
        }
      }
    }

    if (remaining.length > 0) {
      backgroundDownloadCancelRef.current = false
      cacheRemainingAudioFiles(remaining, cacheManager, envConfig.apiBaseUrl).catch(err => {
        if (!backgroundDownloadCancelRef.current) console.error('后台下载出错:', err)
      })
    }
  }

  const cacheRemainingAudioFiles = async (
    remainingSentences: Sentence[],
    cacheManager: FileCacheManager,
    apiBaseUrl: string
  ) => {
    if (backgroundDownloadCancelRef.current) return
    let count = 0
    for (const sentence of remainingSentences) {
      if (backgroundDownloadCancelRef.current) return
      for (const audio of sentence.audios) {
        if (backgroundDownloadCancelRef.current) return
        try {
          await cacheManager.downloadAndCache(audio.id, apiBaseUrl)
          count++
          if (count % 10 === 0) console.log(`后台下载进度: ${count}`)
        } catch (err) {
          if (backgroundDownloadCancelRef.current) return
          console.error(`后台缓存失败 (ID: ${audio.id}):`, err)
        }
      }
    }
  }

  const initAudioPlayer = async (sentencesList: Sentence[]) => {
    if (!sentencesList.length) throw new Error('没有可播放的句子')

    const envConfig = await EnvConfigService.getInstance().getConfig()
    const cacheManager = FileCacheManager.getInstance()
    const audioAdapter = new HtmlAudioAdapter()

    // 重置单例，允许以新 envConfig 重新初始化
    AudioPlayerService.resetInstance()
    const audioService = AudioPlayerService.getInstance(audioAdapter, cacheManager, envConfig)

    audioService.setSentences(sentencesList)
    audioService.setMaxPlayCount(4)
    audioService.resetPlayer()

    if (autoStart && !autoPlayStartedRef.current) {
      autoPlayStartedRef.current = true
      setTimeout(() => {
        try { void audioService.playCurrentAudio() }
        catch (err) { console.error('自动播放失败:', err); autoPlayStartedRef.current = false }
      }, 500)
    }
  }

  const handleReload = () => {
    setInitialized(false)
    setSentences([])
    setError(null)
    initStartedRef.current = false
    void init()
  }

  return (
    <div className="flex flex-col gap-4">
      {/* 加载中 */}
      {loading && (
        <div className="flex flex-col items-center justify-center gap-3 py-16 text-gray-500 dark:text-gray-400">
          <Loader2 size={32} className="animate-spin" />
          <span className="text-sm">{loadingText}</span>
        </div>
      )}

      {/* 错误 */}
      {error && !loading && (
        <div className="flex flex-col items-center gap-3 rounded-lg bg-red-50 p-6 dark:bg-red-900/20">
          <div className="flex items-center gap-2 text-red-600 dark:text-red-400">
            <AlertCircle size={20} />
            <span className="font-medium">加载失败</span>
          </div>
          <p className="text-sm text-red-500 dark:text-red-400">{error}</p>
          <Button variant="outline" size="sm" onClick={handleReload}>
            <RefreshCw size={14} />
            重试
          </Button>
        </div>
      )}

      {/* 已初始化 */}
      {!loading && !error && initialized && (
        <div className="flex items-center gap-2 rounded-md bg-green-50 px-3 py-2 text-sm text-green-700 dark:bg-green-900/20 dark:text-green-400">
          <Music size={16} />
          <span>
            {dataSource === 'annotated' ? '标注播放' : '音频播放'}已就绪 —
            共 {sentences.length} {dataSource === 'annotated' ? '个标注句子' : '个句子'}
          </span>
        </div>
      )}

      {/* 播放器 UI */}
      <AudioPlayer showAnnotation={true} showReport={true} showOptions={true} />
    </div>
  )
}
