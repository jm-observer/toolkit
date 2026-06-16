// 轻量 toast 状态钩子（自造，项目无 toast 库，不加新依赖）。
import { useCallback, useRef, useState } from 'react';

export type ToastTone = 'info' | 'success' | 'error';

export interface ToastItem {
  id: number;
  message: string;
  tone: ToastTone;
}

export interface UseToast {
  toasts: ToastItem[];
  push: (message: string, tone?: ToastTone, durationMs?: number) => void;
  dismiss: (id: number) => void;
}

export function useToast(): UseToast {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const seqRef = useRef(0);
  const timersRef = useRef<Map<number, number>>(new Map());

  const dismiss = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
    const timer = timersRef.current.get(id);
    if (timer !== undefined) {
      window.clearTimeout(timer);
      timersRef.current.delete(id);
    }
  }, []);

  const push = useCallback(
    (message: string, tone: ToastTone = 'info', durationMs = 3200) => {
      const id = ++seqRef.current;
      setToasts((prev) => [...prev, { id, message, tone }]);
      const timer = window.setTimeout(() => dismiss(id), durationMs);
      timersRef.current.set(id, timer);
    },
    [dismiss],
  );

  return { toasts, push, dismiss };
}
