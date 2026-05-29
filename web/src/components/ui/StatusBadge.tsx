import { cn } from '../../lib/cn'
import type { RunStatus } from '../../lib/api'

const styles: Record<RunStatus, string> = {
  draft: 'text-text-faint border-border',
  validated: 'text-text-muted border-border-strong',
  queued: 'text-accent border-accent/40 bg-accent/10',
  running: 'text-accent border-accent/40 bg-accent/10',
  completed: 'text-correct border-correct/40 bg-correct/10',
  failed: 'text-danger border-danger/40 bg-danger/10',
  aborted: 'text-danger border-danger/40 bg-danger/10',
}

export function StatusBadge({ status }: { status: RunStatus }) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded-full border px-2 py-0.5 text-xs font-medium',
        styles[status],
      )}
    >
      {status}
    </span>
  )
}
