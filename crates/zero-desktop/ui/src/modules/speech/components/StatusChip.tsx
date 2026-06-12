import React from 'react';
import { cn } from '../utils';

interface StatusChipProps {
  status: string;
}

export const StatusChip: React.FC<StatusChipProps> = ({ status }) => {
  const configs: Record<string, { label: string; bg: string; text: string; dot: string; pulse?: boolean }> = {
    idle: { label: '就绪', bg: 'rgba(148,163,184,0.16)', text: '#94a3b8', dot: '#94a3b8' },
    initializing: { label: '模型加载中', bg: 'rgba(245,158,11,0.16)', text: '#fbbf24', dot: '#f59e0b' },
    recording: { label: '正在录音', bg: 'rgba(239,68,68,0.18)', text: '#f87171', dot: '#ef4444', pulse: true },
    processing: { label: '处理中', bg: 'rgba(245,158,11,0.16)', text: '#fbbf24', dot: '#f59e0b' },
    error: { label: '异常', bg: 'rgba(239,68,68,0.18)', text: '#f87171', dot: '#ef4444' },
    finished: { label: '已完成', bg: 'rgba(34,197,94,0.16)', text: '#4ade80', dot: '#22c55e' },
  };

  const config = configs[status] || configs.idle;

  return (
    <div
      className="inline-flex items-center gap-2 px-2.5 py-1 rounded-full text-[12px] font-medium transition-colors"
      style={{ backgroundColor: config.bg, color: config.text }}
    >
      <div
        className={cn("w-1.5 h-1.5 rounded-full", config.pulse && "animate-pulse")}
        style={{ backgroundColor: config.dot }}
      />
      {config.label}
    </div>
  );
};
