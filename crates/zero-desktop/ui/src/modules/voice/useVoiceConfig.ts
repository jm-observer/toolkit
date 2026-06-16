// 语音通道配置持久化（plugin-store）。参考 english 模块 EnvConfigService。
import { useCallback, useEffect, useRef, useState } from 'react';
import { Store } from '@tauri-apps/plugin-store';
import { DEFAULT_WAKE_WORDS } from './wakeGate';
import type { VoiceConfig } from './types';

const STORE_FILE = 'voice-channel-config.json';
const KEY = 'config';

export const DEFAULT_VOICE_URL = 'ws://127.0.0.1:8101';

function defaultConfig(): VoiceConfig {
  return {
    url: DEFAULT_VOICE_URL,
    wakeWords: [...DEFAULT_WAKE_WORDS],
    sessionId: '',
  };
}

function coerce(raw: Partial<VoiceConfig> | null | undefined): VoiceConfig {
  const base = defaultConfig();
  if (!raw) return base;
  return {
    url: typeof raw.url === 'string' && raw.url.trim() ? raw.url.trim() : base.url,
    wakeWords:
      Array.isArray(raw.wakeWords) && raw.wakeWords.length > 0
        ? raw.wakeWords.map((w) => String(w)).filter((w) => w.trim().length > 0)
        : base.wakeWords,
    sessionId: typeof raw.sessionId === 'string' ? raw.sessionId : base.sessionId,
  };
}

export interface UseVoiceConfig {
  config: VoiceConfig;
  ready: boolean;
  update: (patch: Partial<VoiceConfig>) => Promise<void>;
}

export function useVoiceConfig(): UseVoiceConfig {
  const [config, setConfig] = useState<VoiceConfig>(defaultConfig);
  const [ready, setReady] = useState(false);
  const storeRef = useRef<Store | null>(null);

  useEffect(() => {
    let canceled = false;
    (async () => {
      try {
        const store = await Store.load(STORE_FILE);
        storeRef.current = store;
        const saved = await store.get<Partial<VoiceConfig>>(KEY);
        if (canceled) return;
        setConfig(coerce(saved));
      } catch (err) {
        console.warn('[voice] load config failed', err);
      } finally {
        if (!canceled) setReady(true);
      }
    })();
    return () => {
      canceled = true;
    };
  }, []);

  const update = useCallback(async (patch: Partial<VoiceConfig>) => {
    setConfig((prev) => {
      const next = coerce({ ...prev, ...patch });
      const store = storeRef.current;
      if (store) {
        store
          .set(KEY, next)
          .then(() => store.save())
          .catch((err) => console.warn('[voice] save config failed', err));
      }
      return next;
    });
  }, []);

  return { config, ready, update };
}
