import type { InputHTMLAttributes, SelectHTMLAttributes, ReactNode } from 'react'
import { cn } from '../../lib/cn'

const field =
  'w-full rounded-[var(--radius)] border border-border bg-surface-2 px-3 py-2 text-sm text-text ' +
  'placeholder:text-text-faint hover:border-border-strong focus:border-accent transition-colors'

export function Field({
  label,
  hint,
  htmlFor,
  children,
}: {
  label: string
  hint?: ReactNode
  htmlFor?: string
  children: ReactNode
}) {
  return (
    <label htmlFor={htmlFor} className="block">
      <span className="mb-1.5 block text-xs font-medium text-text-muted">{label}</span>
      {children}
      {hint && <span className="mt-1 block text-xs text-text-faint">{hint}</span>}
    </label>
  )
}

export function Input({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  // type=password renders as a never-echoed secret field (used for API keys).
  return <input className={cn(field, props.type === 'number' && 'font-mono', className)} {...props} />
}

export function Select({ className, ...props }: SelectHTMLAttributes<HTMLSelectElement>) {
  return <select className={cn(field, 'appearance-none', className)} {...props} />
}
