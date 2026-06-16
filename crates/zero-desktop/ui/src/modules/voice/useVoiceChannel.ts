// zero 语音 WS 客户端。设计：docs/voice-command-agent-design.md §3.2 / §5。
//
// 浏览器原生 WebSocket 长连 + 指数退避重连；连接成功时若有 sessionId 则发一次
// {"type":"switch_session",...}（常驻语音会话，§5.2）；发送为纯文本帧；收帧先
// try JSON.parse → 有 type 按 audio/file/error 信封，否则当纯文本回复。
import { useCallback, useEffect, useRef, useState } from 'react';
import type { VoiceConnState, VoiceReply } from './types';

const BASE_DELAY_MS = 500;
const MAX_DELAY_MS = 15000;

interface Options {
  url: string;
  /** 常驻语音会话 id；空则不发 switch_session。 */
  sessionId: string;
  /** 是否启用连接（关闭时主动断开，不重连）。 */
  enabled: boolean;
  /** 收到归一化回复时回调。 */
  onReply: (reply: VoiceReply) => void;
}

export interface UseVoiceChannel {
  state: VoiceConnState;
  /** 发送一帧纯文本指令；未连接返回 false。 */
  send: (text: string) => boolean;
}

function parseFrame(data: string): VoiceReply {
  try {
    const obj = JSON.parse(data) as unknown;
    if (obj && typeof obj === 'object' && 'type' in obj) {
      const env = obj as { type: string; payload?: unknown };
      const p = (env.payload ?? {}) as Record<string, unknown>;
      if (env.type === 'audio') {
        return {
          kind: 'audio',
          mime: String(p.mime ?? 'audio/wav'),
          dataBase64: String(p.data_base64 ?? ''),
          name: typeof p.name === 'string' ? p.name : undefined,
        };
      }
      if (env.type === 'file') {
        return {
          kind: 'file',
          mime: String(p.mime ?? 'application/octet-stream'),
          name: typeof p.name === 'string' ? p.name : undefined,
          dataBase64: String(p.data_base64 ?? ''),
        };
      }
      if (env.type === 'error') {
        const msg = typeof env.payload === 'string' ? env.payload : JSON.stringify(env.payload);
        return { kind: 'error', message: msg };
      }
      // 未知 type：当文本兜底。
      return { kind: 'text', text: data };
    }
  } catch {
    // 非 JSON → 纯文本回复。
  }
  return { kind: 'text', text: data };
}

export function useVoiceChannel(opts: Options): UseVoiceChannel {
  const { url, sessionId, enabled, onReply } = opts;
  const [state, setState] = useState<VoiceConnState>('idle');

  const wsRef = useRef<WebSocket | null>(null);
  const retryRef = useRef(0);
  const timerRef = useRef<number | null>(null);
  const closedByUsRef = useRef(false);

  // 用 ref 持有最新回调/会话，避免重连闭包陷阱。
  const onReplyRef = useRef(onReply);
  onReplyRef.current = onReply;
  const sessionRef = useRef(sessionId);
  sessionRef.current = sessionId;

  const clearTimer = useCallback(() => {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const connect = useCallback(
    function connectImpl() {
      clearTimer();
      closedByUsRef.current = false;
      let ws: WebSocket;
      try {
        ws = new WebSocket(url);
      } catch (err) {
        console.warn('[voice] WebSocket ctor failed', err);
        scheduleReconnect();
        return;
      }
      wsRef.current = ws;
      setState(retryRef.current > 0 ? 'reconnecting' : 'connecting');

      ws.onopen = () => {
        retryRef.current = 0;
        setState('connected');
        const sid = sessionRef.current.trim();
        if (sid) {
          try {
            ws.send(JSON.stringify({ type: 'switch_session', session_id: sid }));
          } catch (err) {
            console.warn('[voice] switch_session send failed', err);
          }
        }
      };

      ws.onmessage = (ev) => {
        if (typeof ev.data !== 'string') return;
        onReplyRef.current(parseFrame(ev.data));
      };

      ws.onerror = () => {
        // 错误后通常紧跟 onclose，重连交给 onclose 处理。
      };

      ws.onclose = () => {
        if (wsRef.current === ws) wsRef.current = null;
        if (closedByUsRef.current) {
          setState('closed');
          return;
        }
        scheduleReconnect();
      };

      function scheduleReconnect() {
        const attempt = retryRef.current;
        retryRef.current = attempt + 1;
        const delay = Math.min(BASE_DELAY_MS * 2 ** attempt, MAX_DELAY_MS);
        setState('reconnecting');
        clearTimer();
        timerRef.current = window.setTimeout(() => {
          connectImpl();
        }, delay);
      }
    },
    [url, clearTimer],
  );

  useEffect(() => {
    if (!enabled) {
      // 主动关闭，不重连。
      closedByUsRef.current = true;
      clearTimer();
      retryRef.current = 0;
      const ws = wsRef.current;
      wsRef.current = null;
      if (ws) {
        ws.onclose = null;
        ws.onmessage = null;
        ws.onopen = null;
        ws.onerror = null;
        try {
          ws.close();
        } catch {
          /* noop */
        }
      }
      setState('idle');
      return;
    }

    retryRef.current = 0;
    connect();

    return () => {
      closedByUsRef.current = true;
      clearTimer();
      const ws = wsRef.current;
      wsRef.current = null;
      if (ws) {
        ws.onclose = null;
        ws.onmessage = null;
        ws.onopen = null;
        ws.onerror = null;
        try {
          ws.close();
        } catch {
          /* noop */
        }
      }
    };
    // url / enabled 变化时重建连接。
  }, [url, enabled, connect, clearTimer]);

  const send = useCallback((text: string): boolean => {
    const ws = wsRef.current;
    if (!ws || ws.readyState !== WebSocket.OPEN) return false;
    try {
      ws.send(text);
      return true;
    } catch (err) {
      console.warn('[voice] send failed', err);
      return false;
    }
  }, []);

  return { state, send };
}
