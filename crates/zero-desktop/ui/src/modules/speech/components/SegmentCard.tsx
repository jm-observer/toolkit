import React, { useEffect, useRef, useState } from 'react';
import { cn, stripYear } from '../utils';
import type { Segment } from '../api/tauri-client';
import { Button } from './ui/Button';
import { Icon } from './ui/Icon';

interface SegmentCardProps {
  segment: Segment;
  showEnglish?: boolean;
  /** When the dual-model comparison opt-in is on, show the secondary
   *  recognizer's text in a small accent row. Defaults to true so any
   *  segment carrying `text_secondary` is shown — toggling the feature off
   *  for new sessions naturally hides it for future segments. */
  showSecondary?: boolean;
  onCopyChinese: (text: string) => void;
  onCopyEnglish: (text: string) => void;
}

export const SegmentCard: React.FC<SegmentCardProps> = ({
  segment,
  showEnglish,
  showSecondary = true,
  onCopyChinese,
  onCopyEnglish,
}) => {
  const [copiedZh, setCopiedZh] = useState(false);
  const [copiedEn, setCopiedEn] = useState(false);
  const [copiedSec, setCopiedSec] = useState(false);
  const cardRef = useRef<HTMLDivElement | null>(null);
  const maxHeightRef = useRef(0);
  const [minHeight, setMinHeight] = useState<number | undefined>(undefined);

  const handleCopyZh = () => {
    onCopyChinese(segment.text_optimized || segment.text_raw);
    setCopiedZh(true);
    setTimeout(() => setCopiedZh(false), 2000);
  };

  const handleCopyEn = () => {
    onCopyEnglish(segment.text_english || '');
    setCopiedEn(true);
    setTimeout(() => setCopiedEn(false), 2000);
  };

  const handleCopySec = () => {
    if (!segment.text_secondary) return;
    onCopyChinese(segment.text_secondary);
    setCopiedSec(true);
    setTimeout(() => setCopiedSec(false), 2000);
  };

  const showSecondaryRow = showSecondary && !!segment.text_secondary;

  const optimizeRunning = segment.optimize_status === 'running' || segment.optimize_status === 'pending';
  const translateRunning = segment.translate_status === 'running' || segment.translate_status === 'pending';
  const isProcessing = optimizeRunning || translateRunning;
  const duration = segment.end - segment.start;

  useEffect(() => {
    const element = cardRef.current;
    if (!element) {
      return;
    }

    const initialHeight = element.offsetHeight;
    if (initialHeight > maxHeightRef.current) {
      maxHeightRef.current = initialHeight;
      setMinHeight(initialHeight);
    }

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const height = entry.borderBoxSize?.[0]?.blockSize ?? entry.contentRect.height;
        if (height > maxHeightRef.current) {
          maxHeightRef.current = height;
          setMinHeight(height);
        }
      }
    });
    observer.observe(element);

    return () => {
      observer.disconnect();
    };
  }, []);

  return (
    <>
      <div
        ref={cardRef}
        style={minHeight !== undefined ? { minHeight: `${minHeight}px` } : undefined}
        className={cn(
          'group relative flex flex-col p-4 px-4.5 gap-2.5 bg-[var(--bg-card)] border border-[var(--line)] rounded-[16px] shadow-[var(--shadow-sm)] transition-shadow transition-colors animate-fade-up',
          'hover:shadow-[var(--shadow-md)] hover:border-[var(--line-strong)]'
        )}
      >
        <div className="flex items-center gap-3">
          <span className="px-2 py-0.5 rounded-md bg-[var(--bg-soft)] font-mono text-[11px] text-[var(--ink-2)]">
            {stripYear(segment.wall_start)} → {stripYear(segment.wall_end)}
          </span>
          <span className="text-[11px] text-[var(--ink-4)]">{duration.toFixed(1)}s</span>
          {segment.speaker && (
            <span className="px-2 py-0.5 rounded-md bg-[var(--accent-soft,var(--bg-soft))] text-[11px] text-[var(--accent,var(--ink-2))] inline-flex items-center gap-1">
              <Icon name="user" size={11} />
              {segment.speaker}
            </span>
          )}

          <div className="flex-1" />

          <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
            <Button
              variant="ghost"
              size="sm"
              className={cn('h-7 px-2 text-[11px] gap-1.5 transition-colors', copiedZh && 'text-green-600 bg-green-50')}
              disabled={!segment.text_optimized && !segment.text_raw}
              onClick={handleCopyZh}
              title="复制中文"
            >
              <Icon name={copiedZh ? 'check' : 'copy'} size={12} />
              {copiedZh ? '已复制' : '复制中文'}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className={cn('h-7 px-2 text-[11px] gap-1.5 transition-colors', copiedEn && 'text-green-600 bg-green-50')}
              disabled={!segment.text_english}
              onClick={handleCopyEn}
              title="复制英文"
            >
              <Icon name={copiedEn ? 'check' : 'languages'} size={12} />
              {copiedEn ? '已复制' : '复制英文'}
            </Button>
          </div>
        </div>

        <div className="flex flex-col gap-1.5">
          <p className="text-[13px] leading-[1.7] text-[var(--ink-2)] break-words text-pretty">{segment.text_raw}</p>

          {showSecondaryRow && (
            <div className="flex items-start gap-2 px-2.5 py-1.5 rounded-md bg-[var(--bg-soft)] border-l-2 border-[var(--accent,var(--primary))]">
              <span
                className="shrink-0 mt-0.5 text-[10px] uppercase tracking-wider font-medium text-[var(--ink-4)] font-mono"
                title={segment.secondary_kind ? `次模型: ${segment.secondary_kind}` : '次模型'}
              >
                {segment.secondary_kind || '次模型'}
              </span>
              <p className="flex-1 text-[12.5px] leading-[1.6] text-[var(--ink-3)] break-words text-pretty">
                {segment.text_secondary}
              </p>
              <button
                onClick={handleCopySec}
                className="shrink-0 w-6 h-6 rounded flex items-center justify-center text-[var(--ink-4)] hover:text-[var(--primary-deep)] hover:bg-[var(--bg-card)] opacity-0 group-hover:opacity-100 transition-opacity"
                title="复制次模型识别"
              >
                <Icon name={copiedSec ? 'check' : 'copy'} size={11} />
              </button>
            </div>
          )}

          <p className={cn('text-[15px] leading-[1.7] break-words text-pretty', optimizeRunning && 'text-[var(--ink-4)]')}>
            {segment.optimize_status === 'failed'
              ? '优化失败'
              : segment.text_optimized || (optimizeRunning ? '优化中...' : segment.text_raw)}
          </p>

          {showEnglish && (
            <p className={cn('text-[14px] leading-[1.7] break-words text-pretty', translateRunning && 'text-[var(--ink-4)]')}>
              {segment.translate_status === 'failed'
                ? '翻译失败，已保留优化文本'
                : segment.text_english || (translateRunning ? '翻译中...' : '')}
            </p>
          )}
        </div>

        {isProcessing && (
          <div className="absolute top-4 right-4">
            <Icon name="refresh" size={14} className="animate-spin text-[var(--warning)]" />
          </div>
        )}
      </div>
    </>
  );
};
