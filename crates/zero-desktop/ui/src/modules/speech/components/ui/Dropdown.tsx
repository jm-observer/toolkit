import React, { useState } from 'react';
import { Icon } from './Icon';
import { cn } from '../../utils';

interface DropdownProps {
  options: { label: string; value: string }[];
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  label?: string;
  icon?: string;
  className?: string;
}

export const Dropdown: React.FC<DropdownProps> = ({
  options,
  value,
  onChange,
  disabled,
  label,
  icon,
  className
}) => {
  const [isOpen, setIsOpen] = useState(false);
  const selectedOption = options.find(opt => opt.value === value);

  return (
    <div className={cn("relative w-full", className)}>
      {label && <label className="block text-[11.5px] font-medium uppercase tracking-wider text-[var(--ink-3)] mb-1.5 ml-1">{label}</label>}
      <button
        disabled={disabled}
        onClick={() => setIsOpen(!isOpen)}
        className={cn(
          "flex w-full items-center justify-between h-9 px-3.5 bg-[var(--bg-card)] border border-[var(--line)] rounded-lg text-[13px] text-[var(--ink-2)] transition-all",
          "hover:border-[var(--line-strong)] hover:bg-[var(--bg-soft)]",
          "disabled:opacity-50 disabled:cursor-not-allowed",
          isOpen && "ring-2 ring-[var(--primary-soft)] border-[var(--primary)]"
        )}
      >
        <div className="flex items-center gap-2 truncate">
          {icon && <Icon name={icon} size={15} className="text-[var(--ink-3)]" />}
          <span className="truncate">{selectedOption?.label || 'Select...'}</span>
        </div>
        <Icon name="chevron-down" size={14} className={cn("text-[var(--ink-4)] transition-transform", isOpen && "rotate-180")} />
      </button>

      {isOpen && !disabled && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setIsOpen(false)} />
          <div className="absolute top-full left-0 right-0 mt-1.5 z-20 bg-[var(--bg-card)] border border-[var(--line)] rounded-xl shadow-[var(--shadow-md)] overflow-hidden max-h-[280px] overflow-y-auto animate-fade-up">
            {options.map((opt) => (
              <button
                key={opt.value}
                onClick={() => {
                  onChange(opt.value);
                  setIsOpen(false);
                }}
                className={cn(
                  "w-full text-left px-4 py-2.5 text-[13px] transition-colors",
                  opt.value === value ? "bg-[var(--primary-soft)] text-[var(--primary-deep)] font-medium" : "text-[var(--ink-2)] hover:bg-[var(--bg-soft)]"
                )}
              >
                {opt.label}
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
};
