import React from 'react';
import { cn } from '../utils';
import { Waveform } from './Waveform';
import { Icon } from './ui/Icon';

interface RecordCardProps {
  status: string;
  onStart: () => void;
  onStop: () => void;
  onRetry: () => void;
  errorMessage?: string;
  disabled?: boolean;
}

export const RecordCard: React.FC<RecordCardProps> = ({
  status,
  onStart,
  onStop,
  onRetry,
  errorMessage,
  disabled,
}) => {
  const isRecording = status === 'recording' || status === 'processing';
  const isProcessingOnly = status === 'processing';
  const isError = status === 'error';

  const ringColor = isError
    ? '#ef4444'
    : isRecording
    ? (isProcessingOnly ? '#f59e0b' : '#ef4444')
    : 'var(--primary)';

  const handleClick = () => {
    if (isError) return onRetry();
    if (isRecording) return onStop();
    return onStart();
  };

  return (
    <div
      className={cn(
        'relative flex flex-col items-center gap-3 rounded-[16px] p-4',
        'border border-[var(--line)] bg-[var(--bg-card)]'
      )}
    >
      {isRecording && (
        <div className="h-7 flex items-center justify-center w-full">
          <Waveform active intensity={0.6} className="h-7" />
        </div>
      )}

      <button
        type="button"
        onClick={handleClick}
        disabled={disabled || status === 'initializing' || isProcessingOnly}
        className={cn(
          'group inline-flex items-center justify-center gap-2 h-11 px-6 rounded-full',
          'text-white text-[14px] font-medium transition-all duration-150',
          'enabled:hover:brightness-110 enabled:active:scale-[0.98]',
          'disabled:opacity-50 disabled:cursor-not-allowed'
        )}
        style={{
          backgroundColor: ringColor,
          boxShadow: `0 2px 10px ${ringColor}33`,
        }}
      >
        <Icon
          name={isError ? 'refresh' : isRecording ? 'stop' : 'mic'}
          size={17}
          className={cn(isProcessingOnly && 'animate-spin')}
        />
        {isError ? '重试连接' : isRecording ? (isProcessingOnly ? '识别处理中' : '停止识别') : '开始识别'}
      </button>

      {isError && errorMessage && (
        <div className="self-stretch rounded-lg border border-[#ef4444]/40 bg-[#ef4444]/10 px-3 py-2.5 flex flex-col gap-2">
          <div className="flex items-start gap-2 text-[12px] text-[#fca5a5] leading-relaxed">
            <Icon name="alert" size={14} className="mt-0.5 shrink-0" />
            <span className="break-all">{errorMessage}</span>
          </div>
          <button
            type="button"
            onClick={onRetry}
            className="self-start px-3 py-1 rounded-md text-[12px] font-medium bg-[#ef4444]/20 text-[#fca5a5] hover:bg-[#ef4444]/30 transition-colors"
          >
            重试连接
          </button>
        </div>
      )}

      <div className="flex items-center gap-1.5 text-[11px] text-[var(--ink-4)]">
        <span>快捷键</span>
        <kbd className="px-1.5 py-0.5 rounded-md bg-[var(--bg-soft)] border border-[var(--line)] font-mono text-[10px]">⌘</kbd>
        <kbd className="px-1.5 py-0.5 rounded-md bg-[var(--bg-soft)] border border-[var(--line)] font-mono text-[10px]">R</kbd>
      </div>
    </div>
  );
};
