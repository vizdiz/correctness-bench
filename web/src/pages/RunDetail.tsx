import { useEffect, useRef, useState } from 'react'
import { useParams, Link } from 'react-router-dom'
import { api, type RunView, type Tick } from '../lib/api'
import { Button } from '../components/ui/Button'
import { Card, CardBody, CardHeader } from '../components/ui/Card'
import { NumberDisplay } from '../components/ui/NumberDisplay'
import { Sparkline } from '../components/ui/Sparkline'
import { StatusBadge } from '../components/ui/StatusBadge'
import { int, round } from '../lib/format'

const terminal = new Set(['completed', 'failed', 'aborted'])
const MAX_TICKS_HISTORY = 120

export function RunDetail() {
  const { id = '' } = useParams()
  const [run, setRun] = useState<RunView | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [aborting, setAborting] = useState(false)
  const [ticks, setTicks] = useState<Tick[]>([])
  const [streamLive, setStreamLive] = useState(false)
  const esRef = useRef<EventSource | null>(null)

  function load() {
    api.getRun(id).then(setRun).catch((e) => setError(e.message))
  }
  useEffect(load, [id])

  // Live SSE subscription. The connection survives the run; we close on unmount.
  useEffect(() => {
    if (!id) return
    const es = new EventSource(`/v1/runs/${id}/stream`)
    esRef.current = es
    es.addEventListener('open', () => setStreamLive(true))
    es.addEventListener('error', () => setStreamLive(false))
    es.addEventListener('tick', (event) => {
      try {
        const tick = JSON.parse((event as MessageEvent).data) as Tick
        setTicks((prev) => {
          const next = [...prev, tick]
          return next.length > MAX_TICKS_HISTORY ? next.slice(-MAX_TICKS_HISTORY) : next
        })
      } catch {
        /* ignore malformed */
      }
    })
    return () => {
      es.close()
      esRef.current = null
      setStreamLive(false)
    }
  }, [id])

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
  const latest = ticks.length > 0 ? ticks[ticks.length - 1] : null
  const passRate =
    latest && latest.completed_total > 0
      ? (latest.pass_total / latest.completed_total) * 100
      : null
  const correctnessSeries = ticks.map((t) =>
    t.completed_total > 0 ? t.pass_total / t.completed_total : 1,
  )
  const rpsSeries = ticks.map((t) => t.achieved_rps_1s)

  return (
    <div>
      <header className="mb-6 flex items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-3">
            <h1 className="text-xl font-semibold tracking-tight">
              {run.name || <span className="text-text-faint">untitled run</span>}
            </h1>
            <StatusBadge status={run.status} />
            <span
              className={`inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs font-medium ${
                streamLive
                  ? 'border-correct/40 bg-correct/10 text-correct'
                  : 'border-border text-text-faint'
              }`}
              title={streamLive ? 'SSE connected' : 'SSE not connected'}
            >
              <span
                className={`h-1.5 w-1.5 rounded-full ${
                  streamLive ? 'bg-correct' : 'bg-text-faint'
                }`}
              />
              {streamLive ? 'live' : 'idle'}
            </span>
          </div>
          <p className="mt-1 font-mono text-xs text-text-faint">{run.run_id}</p>
        </div>
        {canAbort && (
          <Button variant="danger" onClick={onAbort} disabled={aborting}>
            {aborting ? 'Aborting…' : 'Abort'}
          </Button>
        )}
      </header>

      {/* Stat strip — live values when SSE is delivering ticks, fall back to
          the stored run metadata otherwise. */}
      <Card className="mb-6">
        <CardBody className="grid grid-cols-2 gap-6 sm:grid-cols-4">
          <NumberDisplay label="Target RPS" value={int(run.target_rps)} tone="accent" />
          <NumberDisplay
            label="Achieved RPS"
            value={latest ? round(latest.achieved_rps_1s) : '—'}
            tone="accent"
          />
          <NumberDisplay
            label="Pass rate"
            value={passRate != null ? round(passRate) + '%' : '—'}
            tone={passRate != null && passRate < 95 ? 'danger' : 'correct'}
          />
          <NumberDisplay
            label="Completed"
            value={latest ? int(latest.completed_total) : '—'}
          />
        </CardBody>
      </Card>

      {/* Headline: correctness vs load. Real data once any ticks have arrived;
          placeholder shape until then. */}
      <Card>
        <CardHeader
          title="Correctness vs load"
          subtitle={
            ticks.length > 0
              ? `${ticks.length} live tick(s) · pass=${latest?.pass_total} fail_status=${latest?.fail_status_total}`
              : 'The headline: latency flat while correctness cliffs. Live via SSE.'
          }
        />
        <CardBody>
          <div className="flex flex-col items-center justify-center gap-4 py-10 text-center">
            <Sparkline
              data={correctnessSeries.length > 1 ? correctnessSeries : undefined}
              tone="correct"
              width={420}
              height={64}
            />
            <Sparkline
              data={rpsSeries.length > 1 ? rpsSeries : undefined}
              tone="accent"
              width={420}
              height={32}
            />
            <p className="max-w-md text-xs text-text-faint">
              {ticks.length === 0
                ? 'Waiting for ticks. Start the engine with --push-to and --run-id pointed at this run.'
                : 'Top: correctness (green, 0–100%). Bottom: achieved RPS (blue). When the green line cliffs while blue stays flat — that’s the headline.'}
            </p>
          </div>
        </CardBody>
      </Card>
    </div>
  )
}
