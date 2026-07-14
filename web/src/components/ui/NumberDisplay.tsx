import { cn } from '../../lib/cn'

type Tone = 'default' | 'accent' | 'correct' | 'danger' | 'muted'

const tones: Record<Tone, string> = {
  default: 'text-text',
  accent: 'text-accent',
  correct: 'text-correct',
  danger: 'text-danger',
  muted: 'text-text-muted',
}

/**
 * A large monospaced metric with a label and optional unit - the building block
 * for the stat strip above the (future) charts. Numbers are always pre-rounded
 * by the caller (see lib/format).
 */
export function NumberDisplay({
  label,
  value,
  unit,
  tone = 'default',
}: {
  label: string
  value: string
  unit?: string
  tone?: Tone
}) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-xs font-medium uppercase tracking-wide text-text-faint">
        {label}
      </span>
      <span className={cn('font-mono text-2xl font-medium leading-none', tones[tone])}>
        {value}
        {unit && <span className="ml-1 text-sm text-text-muted">{unit}</span>}
      </span>
    </div>
  )
}
