import type { ButtonHTMLAttributes } from 'react'
import { cn } from '../../lib/cn'

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger'

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant
}

const base =
  'inline-flex items-center justify-center gap-2 rounded-[var(--radius)] px-3.5 py-2 text-sm font-medium ' +
  'transition-colors duration-150 disabled:opacity-50 disabled:pointer-events-none select-none'

const variants: Record<Variant, string> = {
  primary: 'bg-accent text-bg hover:bg-accent-hover',
  secondary: 'bg-surface-2 text-text border border-border-strong hover:border-accent',
  ghost: 'text-text-muted hover:text-text hover:bg-surface-2',
  danger: 'bg-transparent text-danger border border-danger/40 hover:bg-danger/10',
}

export function Button({ variant = 'primary', className, ...props }: Props) {
  return <button className={cn(base, variants[variant], className)} {...props} />
}
