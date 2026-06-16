// 自造 toast host（无第三方依赖）。固定在右下角，点击可关。
import type { ToastItem } from './useToast';

interface ToastHostProps {
  toasts: ToastItem[];
  onDismiss: (id: number) => void;
}

const TONE_STYLE: Record<ToastItem['tone'], { border: string; dot: string }> = {
  info: { border: 'var(--line-strong)', dot: 'var(--primary)' },
  success: { border: 'var(--primary)', dot: 'var(--primary)' },
  error: { border: 'var(--danger)', dot: 'var(--danger)' },
};

export function ToastHost({ toasts, onDismiss }: ToastHostProps) {
  if (toasts.length === 0) return null;
  return (
    <div className="pointer-events-none fixed bottom-4 right-4 z-50 flex flex-col gap-2">
      {toasts.map((t) => {
        const tone = TONE_STYLE[t.tone];
        return (
          <button
            key={t.id}
            onClick={() => onDismiss(t.id)}
            className="pointer-events-auto flex max-w-sm items-start gap-2 rounded-lg px-3 py-2 text-left text-xs shadow-lg transition-opacity hover:opacity-90"
            style={{
              background: 'var(--bg-card)',
              border: `1px solid ${tone.border}`,
              color: 'var(--ink-2)',
              boxShadow: 'var(--shadow-md)',
            }}
            title="点击关闭"
          >
            <span
              className="mt-1 h-1.5 w-1.5 shrink-0 rounded-full"
              style={{ background: tone.dot }}
            />
            <span className="whitespace-pre-wrap break-words leading-relaxed">{t.message}</span>
          </button>
        );
      })}
    </div>
  );
}
