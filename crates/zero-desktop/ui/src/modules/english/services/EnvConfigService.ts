/**
 * English 模块环境配置服务。
 *
 * 改造点（相对原仓）：
 * - apiBaseUrl 不再来自 dev/prod 切换；改为调 Tauri 命令 english_get_g10_base 取。
 * - g10_base / g10_token 不写入 plugin-store（归 app.json，由 cookie 模块管）。
 * - plugin-store 仅存 customerId（english 模块内部 KV）。
 * - 环境切换 UI 整块删除（apiBase 只有一项来源）。
 */

import { Store } from '@tauri-apps/plugin-store'
import { invoke } from '@tauri-apps/api/core'
import type { EnvConfig } from '../types'

class EnvConfigService {
  private static instance: EnvConfigService
  private store: Store | null = null
  private storeReady: Promise<void>

  private constructor() {
    this.storeReady = this._initStore()
  }

  static getInstance(): EnvConfigService {
    if (!EnvConfigService.instance) {
      EnvConfigService.instance = new EnvConfigService()
    }
    return EnvConfigService.instance
  }

  private async _initStore(): Promise<void> {
    try {
      this.store = await Store.load('english-env-config.json')
      // 单用户场景：首次启动时 customerId 缺失则默认填 1（迁自 english/desktop-app）。
      const existing = await this.store.get<number>('customerId')
      if (existing === null || existing === undefined) {
        await this.store.set('customerId', 1)
        await this.store.save()
        console.log('[EnvConfigService] customerId 缺失，已写入默认值 1')
      }
      console.log('[EnvConfigService] store 初始化成功')
    } catch (error) {
      console.error('[EnvConfigService] store 初始化失败:', error)
    }
  }

  /** 等待 store 就绪后返回完整 EnvConfig（含 apiBaseUrl）。 */
  async getConfig(): Promise<EnvConfig> {
    await this.storeReady

    // apiBase 从 app.json 读取（通过 Tauri 命令），不走 plugin-store
    const g10Base: string = await invoke<string>('english_get_g10_base')
    if (!g10Base || !g10Base.trim()) {
      throw new Error('G10 base 未配置，请到设置页配置')
    }

    const config: EnvConfig = { apiBaseUrl: g10Base.trimEnd().replace(/\/$/, '') }

    if (this.store) {
      const customerId = await this.store.get<number>('customerId')
      if (customerId) {
        config.customerId = customerId
      }
    }

    return config
  }

  /** 获取 customer_id（异步）。 */
  async getCustomerId(): Promise<number | undefined> {
    await this.storeReady
    try {
      if (this.store) {
        return await this.store.get<number>('customerId') ?? undefined
      }
    } catch (error) {
      console.error('[EnvConfigService] 获取 customer_id 失败:', error)
    }
    return undefined
  }

  /** 保存 customer_id。 */
  async setCustomerId(customerId: number): Promise<boolean> {
    await this.storeReady
    try {
      if (this.store) {
        await this.store.set('customerId', customerId)
        await this.store.save()
        return true
      }
    } catch (error) {
      console.error('[EnvConfigService] 保存 customer_id 失败:', error)
    }
    return false
  }

  /** 清除 customer_id。 */
  async clearCustomerId(): Promise<boolean> {
    await this.storeReady
    try {
      if (this.store) {
        await this.store.delete('customerId')
        await this.store.save()
        return true
      }
    } catch (error) {
      console.error('[EnvConfigService] 清除 customer_id 失败:', error)
    }
    return false
  }
}

export default EnvConfigService
