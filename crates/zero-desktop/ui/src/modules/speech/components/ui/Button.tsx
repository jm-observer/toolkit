import { type ButtonHTMLAttributes, forwardRef } from 'react';
import { cn } from '../../utils';

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'primary' | 'outline' | 'soft' | 'danger' | 'ghost';
  size?: 'sm' | 'md' | 'lg' | 'icon';
}

const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant = 'primary', size = 'md', ...props }, ref) => {
    const baseStyles = 'inline-flex items-center justify-center rounded-lg transition-all duration-150 active:scale-[0.98] disabled:opacity-50 disabled:pointer-events-none font-medium';

    const variants = {
      primary: 'bg-gradient-to-b from-[var(--primary)] to-[var(--primary-deep)] text-white shadow-[0_4px_14px_rgba(20,161,129,0.22)] hover:shadow-[0_4px_14px_rgba(20,161,129,0.32)] hover:-translate-y-[1px]',
      outline: 'border border-[var(--line)] bg-[var(--bg-card)] text-[var(--ink-2)] hover:bg-[var(--bg-soft)] hover:border-[var(--line-strong)]',
      soft: 'bg-[var(--bg-soft)] text-[var(--ink-2)] hover:bg-[var(--primary-soft)] hover:text-[var(--primary-deep)]',
      danger: 'border border-[var(--danger)] text-[var(--danger)] hover:bg-[var(--danger-soft)]',
      ghost: 'hover:bg-[var(--bg-soft)] text-[var(--ink-2)]',
    };

    const sizes = {
      sm: 'h-7 px-3 text-xs',
      md: 'h-9 px-5 text-[13px]',
      lg: 'h-11 px-6 text-sm',
      icon: 'h-8 w-8',
    };

    return (
      <button
        ref={ref}
        className={cn(baseStyles, variants[variant], sizes[size], className)}
        {...props}
      />
    );
  }
);

Button.displayName = 'Button';

export { Button };
