/**
 * ReplaceAudioModal — 「替换」弹窗：编辑文本 + 选 voice / 语速 → 生成 TTS 预览试听 →
 * 确认后替换当前句子的文本与音频。
 *
 * 流程：必须先「生成预览」试听，才能「确认替换」；改文本/voice/语速会作废已有预览，
 * 需重新生成（保证「听到的 == 存下的」）。
 */

import { useEffect, useState, useCallback } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { X, Play, Loader2 } from 'lucide-react'
import ApiService from '../services/ApiService'
import { Button } from '../../speech/components/ui/Button'

interface VoiceOption {
  id: string
  label: string
}

interface ReplaceAudioModalProps {
  open: boolean
  sentenceId: number
  audioId: number | null
  initialText: string
  onClose: () => void
  /** 替换成功回调，参数为新文本（父组件据此刷新播放器状态）。 */
  onReplaced: (newText: string) => void
}

/** 把 /voices 各种可能形态归一化成 {id,label} 列表。 */
function normalizeVoices(raw: any): VoiceOption[] {
  const toOpt = (v: any): VoiceOption | null => {
    if (typeof v === 'string') return { id: v, label: v }
    if (v && typeof v === 'object') {
      const id = v.id ?? v.voice_id ?? v.name ?? v.value
      if (id != null) return { id: String(id), label: String(v.name ?? v.label ?? id) }
    }
    return null
  }
  let list: any[] = []
  if (Array.isArray(raw)) list = raw
  else if (raw && typeof raw === 'object') {
    if (Array.isArray(raw.voices)) list = raw.voices
    else if (Array.isArray(raw.data)) list = raw.data
    else if (Array.isArray(raw.spk)) list = raw.spk
  }
  return list.map(toOpt).filter((x): x is VoiceOption => x !== null)
}

export default function ReplaceAudioModal({
  open, sentenceId, audioId, initialText, onClose, onReplaced
}: ReplaceAudioModalProps) {
  const [text, setText] = useState(initialText)
  const [voices, setVoices] = useState<VoiceOption[]>([])
  const [voiceId, setVoiceId] = useState('')
  const [speed, setSpeed] = useState(1.0)
  const [previewPath, setPreviewPath] = useState<string | null>(null)
  const [previewSrc, setPreviewSrc] = useState<string | null>(null)
  const [generating, setGenerating] = useState(false)
  const [replacing, setReplacing] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // 打开时重置 + 拉音色库。
  useEffect(() => {
    if (!open) return
    setText(initialText)
    setSpeed(1.0)
    setPreviewPath(null)
    setPreviewSrc(null)
    setError(null)
    let cancelled = false
    ApiService.getInstance().getVoices()
      .then(raw => {
        if (cancelled) return
        const opts = normalizeVoices(raw)
        setVoices(opts)
        setVoiceId(prev => prev || (opts[0]?.id ?? ''))
      })
      .catch(e => { if (!cancelled) setError('加载音色失败：' + (e?.message || e)) })
    return () => { cancelled = true }
  }, [open, initialText])

  // 任何会改变音频内容的参数变化 → 作废已生成的预览。
  const invalidatePreview = useCallback(() => {
    setPreviewPath(null)
    setPreviewSrc(null)
  }, [])

  const handleGenerate = async () => {
    if (!text.trim()) { setError('文本不能为空'); return }
    setGenerating(true)
    setError(null)
    try {
      const path = await ApiService.getInstance().previewTts(text.trim(), voiceId, speed)
      setPreviewPath(path)
      // 带时间戳击穿 webview 资源缓存，确保每次试听都是最新生成的。
      setPreviewSrc(convertFileSrc(path) + '?t=' + Date.now())
    } catch (e: any) {
      setError(e?.message || String(e))
      invalidatePreview()
    } finally {
      setGenerating(false)
    }
  }

  const handleConfirm = async () => {
    if (audioId == null) { setError('当前句子没有可替换的音频'); return }
    if (!previewPath) { setError('请先生成预览并试听'); return }
    setReplacing(true)
    setError(null)
    try {
      await ApiService.getInstance().replaceSentenceAudio(sentenceId, audioId, text.trim(), previewPath)
      onReplaced(text.trim())
      onClose()
    } catch (e: any) {
      setError(e?.message || String(e))
    } finally {
      setReplacing(false)
    }
  }

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-lg rounded-xl bg-white p-5 shadow-xl dark:bg-gray-900"
        onClick={e => e.stopPropagation()}
      >
        <div className="mb-4 flex items-center justify-between">
          <h3 className="text-base font-semibold text-gray-800 dark:text-gray-100">替换句子音频</h3>
          <button
            className="rounded-md p-1 text-gray-400 hover:bg-gray-100 hover:text-gray-600 dark:hover:bg-gray-800"
            onClick={onClose}
            title="关闭"
          >
            <X size={18} />
          </button>
        </div>

        {error && (
          <div className="mb-3 rounded-md bg-red-50 px-3 py-2 text-sm text-red-600 dark:bg-red-900/30 dark:text-red-400">
            {error}
          </div>
        )}

        {/* 文本 */}
        <label className="mb-1 block text-xs text-gray-500 dark:text-gray-400">文本</label>
        <textarea
          value={text}
          onChange={e => { setText(e.target.value); invalidatePreview() }}
          rows={3}
          className="mb-3 w-full resize-y rounded-lg border border-gray-300 px-3 py-2 text-sm text-gray-800 focus:border-blue-500 focus:outline-none dark:border-gray-700 dark:bg-gray-800 dark:text-gray-100"
          placeholder="输入要朗读的英文文本"
        />

        {/* voice + 语速 */}
        <div className="mb-3 flex gap-3">
          <div className="flex-1">
            <label className="mb-1 block text-xs text-gray-500 dark:text-gray-400">音色</label>
            {voices.length > 0 ? (
              <select
                value={voiceId}
                onChange={e => { setVoiceId(e.target.value); invalidatePreview() }}
                className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm text-gray-800 focus:border-blue-500 focus:outline-none dark:border-gray-700 dark:bg-gray-800 dark:text-gray-100"
              >
                {voices.map(v => <option key={v.id} value={v.id}>{v.label}</option>)}
              </select>
            ) : (
              <input
                type="text"
                value={voiceId}
                onChange={e => { setVoiceId(e.target.value); invalidatePreview() }}
                className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm text-gray-800 focus:border-blue-500 focus:outline-none dark:border-gray-700 dark:bg-gray-800 dark:text-gray-100"
                placeholder="voice id"
              />
            )}
          </div>
          <div className="w-40">
            <label className="mb-1 block text-xs text-gray-500 dark:text-gray-400">语速 {speed.toFixed(1)}x</label>
            <input
              type="range"
              min={0.5}
              max={2.0}
              step={0.1}
              value={speed}
              onChange={e => { setSpeed(parseFloat(e.target.value)); invalidatePreview() }}
              className="w-full"
            />
          </div>
        </div>

        {/* 预览 */}
        <div className="mb-4 flex items-center gap-3">
          <Button variant="outline" size="sm" onClick={handleGenerate} disabled={generating || !text.trim()}>
            {generating ? <Loader2 size={14} className="animate-spin" /> : <Play size={14} />}
            {generating ? '生成中...' : '生成预览'}
          </Button>
          {previewSrc && (
            <audio key={previewSrc} src={previewSrc} controls className="h-8 flex-1" />
          )}
        </div>

        {/* 操作 */}
        <div className="flex justify-end gap-2">
          <Button variant="outline" size="sm" onClick={onClose} disabled={replacing}>取消</Button>
          <Button
            variant="primary"
            size="sm"
            onClick={handleConfirm}
            disabled={replacing || !previewPath || audioId == null}
            title={!previewPath ? '请先生成预览并试听' : undefined}
          >
            {replacing ? <Loader2 size={14} className="animate-spin" /> : null}
            {replacing ? '替换中...' : '确认替换'}
          </Button>
        </div>
      </div>
    </div>
  )
}
