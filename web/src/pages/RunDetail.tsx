import { useEffect, useState } from 'react'
import { useParams, Link } from 'react-router-dom'
import { api, type RunView } from '../lib/api'
import { Button } from '../components/ui/Button'
import { Card, CardBody, CardHeader } from '../components/ui/Card'
import { NumberDisplay } from '../components/ui/NumberDisplay'
import { Sparkline } from '../components/ui/Sparkline'
import { StatusBadge } from '../components/ui/StatusBadge'
import { int, round, shortTime } from '../lib/format'

const terminal = new Set(['completed', 'failed', 'aborted'])

export function RunDetail() {
  const { id = '' } = useParams()
  const [run, setRun] = useState<RunView | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [aborting, setAborting] = useState(false)

  function load() {
    api.getRun(id).then(setRun).catch((e) => setError(e.message))
  }
  useEffect(load, [id])

  async function onAbort() {
    setAborting(true)
    try {
      await api.abortRun(id)
      load()
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setAborting(false)
    }
  }

  if (error) {
    return (
      <Card className="border-danger/40 bg-danger/5 px-5 py-4 text-sm text-danger">
        {error} — <Link to="/runs" className="underline">back to runs</Link>
      </Card>
    )
  }
  if (!run) return <p className="text-sm text-text-faint">Loading…</p>

  const canAbort = !terminal.has(run.status)

  return (
    <div>
      <header className="mb-6 flex items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-3">
            <h1 className="text-xl font-semibold tracking-tight">
              {run.name || <span className="text-text-faint">untitled run</span>}
            </h1>
            <StatusBadge status={run.status} />
          </div>
          <p className="mt-1 font-mono text-xs text-text-faint">{run.run_id}</p>
        </div>
        {canAbort && (
          <Button variant="danger" onClick={onAbort} disabled={aborting}>
            {aborting ? 'Aborting…' : 'Abort'}
          </Button>
        )}
      </header>

      {/* Stat strip — populated with placeholders until the engine streams data. */}
      <Card className="mb-6">
        <CardBody className="grid grid-cols-2 gap-6 sm:grid-cols-4">
          <NumberDisplay label="Target RPS" value={int(run.target_rps)} tone="accent" />
          <NumberDisplay
            label="Effective RPS"
            value={run.effective_rps != null ? round(run.effective_rps) : '—'}
            tone="accent"
          />
          <NumberDisplay label="Duration" value={run.duration_s != null ? String(run.duration_s) : '—'} unit="s" />
          <NumberDisplay label="Created" value={shortTime(run.created_at)} tone="muted" />
        </CardBody>
      </Card>

      {/* Headline chart placeholder — the cliff goes here. */}
      <Card>
        <CardHeader
          title="Correctness vs load"
          subtitle="The headline: latency flat (blue) while correctness cliffs (green). Wired to live SSE once the engine lands."
        />
        <CardBody>
          <div className="flex flex-col items-center justify-center gap-4 py-14 text-center">
            <Sparkline data={[1, 1, 1, 1, 0.95, 0.6, 0.2, 0.05]} tone="correct" width={260} height={56} />
            <Sparkline data={[0.5, 0.5, 0.52, 0.5, 0.51, 0.5, 0.52, 0.5]} tone="accent" width={260} height={28} />
            <p className="max-w-sm text-xs text-text-faint">
              Chart placeholder. The real headline overlays correctness % and
              latency percentiles on a shared offered-RPS axis, live via SSE.
            </p>
          </div>
        </CardBody>
      </Card>
    </div>
  )
}
