import { useEffect, useMemo, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api, type RunStatus, type RunSummary } from '../lib/api'
import { Button } from '../components/ui/Button'
import { Card } from '../components/ui/Card'
import { Input } from '../components/ui/Input'
import { StatusBadge } from '../components/ui/StatusBadge'
import { cn } from '../lib/cn'
import { int, shortTime } from '../lib/format'

type StatusFilter = 'all' | RunStatus
const STATUS_OPTIONS: StatusFilter[] = [
  'all',
  'running',
  'queued',
  'completed',
  'failed',
  'aborted',
]

export function RunsList() {
  const [runs, setRuns] = useState<RunSummary[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all')
  const [nameQuery, setNameQuery] = useState('')
  const [selected, setSelected] = useState<string[]>([])
  const nav = useNavigate()

  useEffect(() => {
    setRuns(null)
    setError(null)
    setSelected([])
    api
      .listRuns({ limit: 50, status: statusFilter === 'all' ? undefined : statusFilter })
      .then((r) => setRuns(r.runs))
      .catch((e) => setError(e.message))
  }, [statusFilter])

  const filtered = useMemo(() => {
    if (!runs) return runs
    if (!nameQuery.trim()) return runs
    const q = nameQuery.trim().toLowerCase()
    return runs.filter((r) =>
      (r.name ?? '').toLowerCase().includes(q) || r.run_id.includes(q),
    )
  }, [runs, nameQuery])

  function toggle(id: string) {
    setSelected((cur) => {
      if (cur.includes(id)) return cur.filter((x) => x !== id)
      // Cap selection at 2 - we only compare pairs.
      if (cur.length >= 2) return [cur[1], id]
      return [...cur, id]
    })
  }

  const canCompare = selected.length === 2
  return (
    <div>
      <header className="mb-6 flex items-end justify-between">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Runs</h1>
          <p className="mt-1 max-w-xl text-sm text-text-muted">
            Find the load at which an API's responses start going wrong — while its latency still looks healthy.
          </p>
        </div>
        <Link to="/runs/new">
          <Button>New run</Button>
        </Link>
      </header>

      <Card className="mb-4 flex flex-wrap items-center gap-3 px-4 py-3">
        <div className="inline-flex rounded-md border border-border bg-surface-2/30 p-0.5 text-xs">
          {STATUS_OPTIONS.map((s) => (
            <button
              key={s}
              onClick={() => setStatusFilter(s)}
              className={cn(
                'rounded px-2.5 py-1 capitalize transition-colors',
                statusFilter === s
                  ? 'bg-accent/15 text-accent'
                  : 'text-text-faint hover:text-text-muted',
              )}
            >
              {s}
            </button>
          ))}
        </div>
        <div className="min-w-[200px] flex-1">
          <Input
            type="search"
            value={nameQuery}
            onChange={(e) => setNameQuery(e.target.value)}
            placeholder="Filter by name or run-id…"
          />
        </div>
        {selected.length > 0 && (
          <div className="flex items-center gap-2 text-xs">
            <span className="text-text-muted">{selected.length} selected</span>
            <Button
              variant="secondary"
              disabled={!canCompare}
              onClick={() => nav(`/compare/${selected[0]}/${selected[1]}`)}
            >
              Compare A → B
            </Button>
            <Button variant="ghost" onClick={() => setSelected([])}>
              Clear
            </Button>
          </div>
        )}
      </Card>

      {error && (
        <Card className="border-danger/40 bg-danger/5 px-5 py-4 text-sm text-danger">
          Couldn’t reach the control plane: {error}
        </Card>
      )}

      {!error && filtered && filtered.length === 0 && runs && runs.length === 0 && (
        <Card className="px-5 py-12 text-center">
          <p className="text-sm text-text-muted">No runs yet.</p>
          <Link to="/runs/new" className="mt-3 inline-block">
            <Button variant="secondary">Create your first run</Button>
          </Link>
        </Card>
      )}

      {!error && filtered && filtered.length === 0 && runs && runs.length > 0 && (
        <Card className="px-5 py-8 text-center text-sm text-text-muted">
          No runs match this filter.
        </Card>
      )}

      {!error && filtered && filtered.length > 0 && (
        <Card>
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-text-faint">
                <th className="px-3 py-3 font-medium" aria-label="select" />
                <th className="px-5 py-3 font-medium">Name</th>
                <th className="px-5 py-3 font-medium">Status</th>
                <th className="px-5 py-3 text-right font-medium">Target RPS</th>
                <th className="px-5 py-3 text-right font-medium">Duration</th>
                <th className="px-5 py-3 text-right font-medium">Created</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((r) => {
                const picked = selected.includes(r.run_id)
                return (
                  <tr
                    key={r.run_id}
                    className={cn(
                      'border-b border-border/60 last:border-0 hover:bg-surface-2/40',
                      picked && 'bg-accent/5',
                    )}
                  >
                    <td className="px-3 py-3">
                      <input
                        type="checkbox"
                        checked={picked}
                        onChange={() => toggle(r.run_id)}
                        aria-label={`select ${r.name ?? r.run_id}`}
                      />
                    </td>
                    <td className="px-5 py-3">
                      <Link to={`/runs/${r.run_id}`} className="text-text hover:text-accent">
                        {r.name || <span className="text-text-faint">untitled</span>}
                      </Link>
                    </td>
                    <td className="px-5 py-3">
                      <StatusBadge status={r.status} />
                    </td>
                    <td className="px-5 py-3 text-right font-mono">{int(r.target_rps)}</td>
                    <td className="px-5 py-3 text-right font-mono text-text-muted">{r.duration_s}s</td>
                    <td className="px-5 py-3 text-right text-text-muted">{shortTime(r.created_at)}</td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </Card>
      )}

      {!error && !runs && <p className="text-sm text-text-faint">Loading…</p>}
    </div>
  )
}
