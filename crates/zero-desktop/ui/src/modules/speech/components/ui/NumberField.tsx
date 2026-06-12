import React, { useEffect, useState } from 'react';
import { Icon } from './Icon';
import { cn } from '../../utils';

interface NumberFieldProps {
  value: number;
  onChange: (value: number) => void;
  disabled?: boolean;
  label?: string;
  icon?: string;
  min?: number;
  max?: number;
  step?: number;
  suffix?: string;
  className?: string;
}

export const NumberField: React.FC<NumberFieldProps> = ({
  value,
  onChange,
  disabled,
  label,
  icon,
  min = 0,
  max = Number.POSITIVE_INFINITY,
  step = 1,
  suffix,
  className,
}) => {
  // Local draft text so the user can clear/retype freely; the clamped value
  // is only committed back to the parent on blur or Enter.
  const [text, setText] = useState(String(value));
  const [focused, setFocused] = useState(false);

  useEffect(() => {
    if (!focused) setText(String(value));
  }, [value, focused]);

  const commit = () => {
    const parsed = Number(text);
    const clamped = Number.isFinite(parsed)
      ? Math.min(max, Math.max(min, parsed))
      : value;
    setText(String(clamped));
    if (clamped !== value) onChange(clamped);
  };

  return (
    <div className={cn('relative w-full', className)}>
      {label && (
        <label className="block text-[11.5px] font-medium uppercase tracking-wider text-[var(--ink-3)] mb-1.5 ml-1">
          {label}
        </label>
      )}
      <div
        className={cn(
          'flex w-full items-center gap-2 h-9 px-3.5 bg-[var(--bg-card)] border border-[var(--line)] rounded-lg text-[13px] text-[var(--ink-2)] transition-all',
          'hover:border-[var(--line-strong)]',
          'focus-within:ring-2 focus-within:ring-[var(--primary-soft)] focus-within:border-[var(--primary)]',
          disabled && 'opacity-50 cursor-not-allowed',
        )}
      >
        {icon && <Icon name={icon} size={15} className="text-[var(--ink-3)] shrink-0" />}
        <input
          type="number"
          inputMode="decimal"
          value={text}
          min={min}
          max={Number.isFinite(max) ? max : undefined}
          step={step}
          disabled={disabled}
          onChange={(e) => setText(e.target.value)}
          onFocus={() => setFocused(true)}
          onBlur={() => {
            setFocused(false);
            commit();
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
          }}
          className="flex-1 min-w-0 bg-transparent outline-none text-[var(--ink-2)] disabled:cursor-not-allowed"
        />
        {suffix && <span className="text-[var(--ink-4)] shrink-0">{suffix}</span>}
      </div>
    </div>
  );
};
