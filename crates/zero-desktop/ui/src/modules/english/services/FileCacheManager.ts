/**
 * FileCacheManager — Tauri FS 实现的音频缓存管理器。
 *
 * 改造点（相对原仓）：
 * - 缓存根路径改为调 Tauri 命令 english_get_audio_cache_dir 取，
 *   指向 {workspace}/english/audio-cache/，不再使用 appDataDir。
 */

import { exists, mkdir, writeFile, remove, readDir } from '@tauri-apps/plugin-fs'
import { join } from '@tauri-apps/api/path'
import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import type { CacheStats, CacheItem } from '../types'
import ApiService from './ApiService'

class FileCacheManager {
  private static instance: FileCacheManager
  private isInitialized: boolean = false
  private cacheRoot: string = ''

  private constructor() {
    void this.init()
  }

  static getInstance(): FileCacheManager {
    if (!FileCacheManager.instance) {
      FileCacheManager.instance = new FileCacheManager()
    }
    return FileCacheManager.instance
  }

  private async init(): Promise<void> {
    if (this.isInitialized) return
    try {
      // 缓存目录由 Tauri 后端从 workspace 取，不依赖 appDataDir
      const cacheDir: string = await invoke<string>('english_get_audio_cache_dir')
      this.cacheRoot = cacheDir
      if (!(await exists(this.cacheRoot))) {
        await mkdir(this.cacheRoot, { recursive: true })
      }
      this.isInitialized = true
      console.log('[FileCacheManager] 缓存目录:', this.cacheRoot)
    } catch (error) {
      console.error('[FileCacheManager] 初始化失败:', error)
    }
  }

  private async ensureReady(): Promise<void> {
    if (!this.isInitialized) await this.init()
  }

  private async buildPathForKey(key: string): Promise<string> {
    await this.ensureReady()
    return join(this.cacheRoot, `${key}.mp3`)
  }

  private async buildFallbackPath(key: string): Promise<string> {
    await this.ensureReady()
    const last = key.includes('_') ? key.split('_').pop()! : key
    return join(this.cacheRoot, `${last}.mp3`)
  }

  async getCache(key: string): Promise<string | null> {
    const p = await this.buildPathForKey(key)
    if (await exists(p)) return convertFileSrc(p)
    const fp = await this.buildFallbackPath(key)
    if (await exists(fp)) return convertFileSrc(fp)
    return null
  }

  async removeCache(key: string): Promise<boolean> {
    const p = await this.buildPathForKey(key)
    if (await exists(p)) { await remove(p); return true }
    const fp = await this.buildFallbackPath(key)
    if (await exists(fp)) { await remove(fp); return true }
    return false
  }

  async clearAllCache(): Promise<void> {
    await this.ensureReady()
    const entries = await readDir(this.cacheRoot)
    for (const e of entries) {
      if (e.isFile) {
        const filePath = await join(this.cacheRoot, e.name)
        try { await remove(filePath) } catch { /* ignore */ }
      }
    }
  }

  getStats(): CacheStats {
    return { totalSize: 0, totalCount: 0, items: [] as CacheItem[] }
  }

  async downloadAndCache(audioId: number, _apiBaseUrl: string): Promise<string> {
    await this.ensureReady()
    const key = String(audioId)
    const dest = await this.buildPathForKey(key)
    if (!(await exists(dest))) {
      console.log(`[FileCacheManager] 下载音频: ${audioId}`)
      const buf = await ApiService.getInstance().downloadAudio(audioId)
      await writeFile(dest, new Uint8Array(buf))
      console.log(`[FileCacheManager] 已保存: ${audioId}`)
    }
    return convertFileSrc(dest)
  }

  async removeStringCache(key: string): Promise<boolean> {
    await this.ensureReady()
    const dataDir = await join(this.cacheRoot, '..', 'data')
    const path = await join(dataDir, `${key}.json`)
    try {
      if (await exists(path)) { await remove(path); return true }
    } catch { /* ignore */ }
    return false
  }
}

export default FileCacheManager
