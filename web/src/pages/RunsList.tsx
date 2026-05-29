import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api, type RunSummary } from '../lib/api'
import { Button } from '../components/ui/Button'
import { Card } from '../components/ui/Card'
import { StatusBadge } from '../components/ui/StatusBadge'
import { int, shortTime } from '../lib/format'

export function RunsList() {
  const [runs, setRuns] = useState<RunSummary[] | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    api
      .listRuns({ limit: 50 })
      .then((r) => setRuns(r.runs))
      .catch((e) => setError(e.message))
  }, [])

  return (
    <div>
      <header className="mb-6 flex items-end justify-between">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Runs</h1>
          <p className="mt-1 text-sm text-text-muted">
            Every benchmark execution. The cliff lives in the details view.
          </p>
        </div>
        <Link to="/runs/new">
          <Button>New run</Button>
        </Link>
      </header>

      {error && (
        <Card className="border-danger/40 bg-danger/5 px-5 py-4 text-sm text-danger">
          Couldn’t reach the control plane: {error}
        </Card>
      )}

      {!error && runs && runs.length === 0 && (
        <Card className="px-5 py-12 text-center">
          <p className="text-sm text-text-muted">No runs yet.</p>
          <Link to="/runs/new" className="mt-3 inline-block">
            <Button variant="secondary">Create your first run</Button>
          </Link>
        </Card>
      )}

      {!error && runs && runs.length > 0 && (
        <Card>
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-text-faint">
                <th className="px-5 py-3 font-medium">Name</th>
                <th className="px-5 py-3 font-medium">Status</th>
                <th className="px-5 py-3 text-right font-medium">Target RPS</th>
                <th className="px-5 py-3 text-right font-medium">Duration</th>
                <th className="px-5 py-3 text-right font-medium">Created</th>
              </tr>
            </thead>
            <tbody>
              {runs.map((r) => (
                <tr key={r.run_id} className="border-b border-border/60 last:border-0 hover:bg-surface-2/40">
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
              ))}
            </tbody>
          </table>
        </Card>
      )}

      {!error && !runs && <p className="text-sm text-text-faint">Loading…</p>}
    </div>
  )
}
