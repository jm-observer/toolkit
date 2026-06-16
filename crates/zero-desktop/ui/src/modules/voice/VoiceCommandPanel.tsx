// 语音指令面板。设计：docs/voice-command-agent-design.md §3.3 / §7。
// 显示连接状态 + 可改地址/唤醒词 + 成对「指令 → 回复」记录。
import { useEffect, useState } from 'react';
import type { VoiceConnState, VoiceConfig, VoiceExchange, VoiceReply } from './types';

interface Props {
  state: VoiceConnState;
  config: VoiceConfig;
  exchanges: VoiceExchange[];
  onUpdateConfig: (patch: Partial<VoiceConfig>) => void;
}

const STATE_LABEL: Record<VoiceConnState, { text: string; color: string }> = {
  idle: { text: '未连接', color: 'var(--ink-4)' },
  connecting: { text: '连接中…', color: 'var(--warning)' },
  connected: { text: '已连接', color: 'var(--primary)' },
  reconnecting: { text: '重连中…', color: 'var(--warning)' },
  closed: { text: '已关闭', color: 'var(--ink-4)' },
};

function replyText(r: VoiceReply): string {
  switch (r.kind) {
    case 'text':
      return r.text;
    case 'audio':
      return `[语音回复 ${r.mime}]`;
    case 'file':
      return `[文件 ${r.name ?? r.mime}]`;
    case 'error':
      return `⚠ ${r.message}`;
  }
}

export function VoiceCommandPanel({ state, config, exchanges, onUpdateConfig }: Props) {
  const [urlDraft, setUrlDraft] = useState(config.url);
  const [wakeDraft, setWakeDraft] = useState(config.wakeWords.join(', '));
  const status = STATE_LABEL[state];

  // config 外部变化（异步加载完成）时同步草稿。
  useEffect(() => {
    setUrlDraft(config.url);
  }, [config.url]);
  useEffect(() => {
    setWakeDraft(config.wakeWords.join(', '));
  }, [config.wakeWords]);

  const commitUrl = () => {
    const next = urlDraft.trim();
    if (next && next !== config.url) onUpdateConfig({ url: next });
  };

  const commitWake = () => {
    const words = wakeDraft
      .split(/[,，\s]+/)
      .map((w) => w.trim())
      .filter((w) => w.length > 0);
    if (words.length > 0 && words.join('') !== config.wakeWords.join('')) {
      onUpdateConfig({ wakeWords: words });
    }
  };

  return (
    <div
      className="flex flex-col gap-3 rounded-lg p-3"
      style={{ background: 'var(--bg-card)', border: '1px solid var(--line)' }}
    >
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <span className="text-xs font-semibold" style={{ color: 'var(--ink-2)' }}>
            语音指令通道
          </span>
        </div>
        <div className="flex items-center gap-1.5">
          <span
            className="h-2 w-2 rounded-full"
            style={{ background: status.color }}
          />
          <span className="text-[11px]" style={{ color: status.color }}>
            {status.text}
          </span>
        </div>
      </div>

      <div className="flex flex-col gap-2">
        <label className="flex flex-col gap-1">
          <span className="text-[11px]" style={{ color: 'var(--ink-4)' }}>
            zero WS 地址
          </span>
          <input
            value={urlDraft}
            onChange={(e) => setUrlDraft(e.target.value)}
            onBlur={commitUrl}
            onKeyDown={(e) => {
              if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
            }}
            spellCheck={false}
            className="h-7 rounded-md px-2 text-xs outline-none"
            style={{
              background: 'var(--bg-soft)',
              border: '1px solid var(--line)',
              color: 'var(--ink-2)',
            }}
            placeholder="ws://127.0.0.1:8101"
          />
        </label>

        <label className="flex flex-col gap-1">
          <span className="text-[11px]" style={{ color: 'var(--ink-4)' }}>
            唤醒词（逗号分隔）
          </span>
          <input
            value={wakeDraft}
            onChange={(e) => setWakeDraft(e.target.value)}
            onBlur={commitWake}
            onKeyDown={(e) => {
              if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
            }}
            spellCheck={false}
            className="h-7 rounded-md px-2 text-xs outline-none"
            style={{
              background: 'var(--bg-soft)',
              border: '1px solid var(--line)',
              color: 'var(--ink-2)',
            }}
            placeholder="zero, 泽罗, 零"
          />
        </label>
      </div>

      <div className="flex flex-col gap-2">
        <span className="text-[11px]" style={{ color: 'var(--ink-4)' }}>
          指令记录
        </span>
        {exchanges.length === 0 ? (
          <p className="py-3 text-center text-[11px]" style={{ color: 'var(--ink-4)' }}>
            说「zero，……」即可向 agent 发送指令
          </p>
        ) : (
          <div className="flex max-h-72 flex-col gap-2 overflow-y-auto pr-1">
            {exchanges
              .slice()
              .reverse()
              .map((ex) => (
                <div
                  key={ex.id}
                  className="flex flex-col gap-1 rounded-md p-2"
                  style={{ background: 'var(--bg-soft)', border: '1px solid var(--line)' }}
                >
                  <div className="flex items-start gap-1.5">
                    <span
                      className="shrink-0 text-[10px] font-semibold"
                      style={{ color: 'var(--primary)' }}
                    >
                      我
                    </span>
                    <span className="text-xs leading-relaxed" style={{ color: 'var(--ink-2)' }}>
                      {ex.command}
                    </span>
                  </div>
                  {ex.replies.length === 0 ? (
                    <div className="flex items-center gap-1.5">
                      <span
                        className="shrink-0 text-[10px] font-semibold"
                        style={{ color: 'var(--ink-4)' }}
                      >
                        zero
                      </span>
                      <span className="text-[11px] italic" style={{ color: 'var(--ink-4)' }}>
                        等待回复…
                      </span>
                    </div>
                  ) : (
                    ex.replies.map((r, i) => (
                      <div key={i} className="flex items-start gap-1.5">
                        <span
                          className="shrink-0 text-[10px] font-semibold"
                          style={{
                            color: r.kind === 'error' ? 'var(--danger)' : 'var(--ink-3)',
                          }}
                        >
                          zero
                        </span>
                        <span
                          className="whitespace-pre-wrap break-words text-xs leading-relaxed"
                          style={{
                            color: r.kind === 'error' ? 'var(--danger)' : 'var(--ink-2)',
                          }}
                        >
                          {replyText(r)}
                        </span>
                      </div>
                    ))
                  )}
                </div>
              ))}
          </div>
        )}
      </div>
    </div>
  );
}
