import { useEffect, useRef, useState } from 'react'
import { useParams, Link } from 'react-router-dom'
import { api, type RunView, type Tick } from '../lib/api'
import { Button } from '../components/ui/Button'
import { Card, CardBody, CardHeader } from '../components/ui/Card'
import { NumberDisplay } from '../components/ui/NumberDisplay'
import { StatusBadge } from '../components/ui/StatusBadge'
import { HeadlineChart } from '../components/charts/HeadlineChart'
import { int, round, usToMs } from '../lib/format'

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
  // Per-tick pass rate is the headline (this 1 s window, not cumulative).
  const passRate =
    latest && latest.this_tick.total > 0
      ? (latest.this_tick.pass / latest.this_tick.total) * 100
      : null

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

      {/* Stat strip — live values when SSE is delivering ticks. Latency is the
          flat axis of the headline; pass rate is the cliff. */}
      <Card className="mb-6">
        <CardBody className="grid grid-cols-2 gap-6 sm:grid-cols-5">
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
            label="Latency p50"
            value={latest ? usToMs(latest.percentiles_so_far.p50_us) : '—'}
            unit="ms"
            tone="accent"
          />
          <NumberDisplay
            label="Latency p99"
            value={latest ? usToMs(latest.percentiles_so_far.p99_us) : '—'}
            unit="ms"
            tone="accent"
          />
        </CardBody>
      </Card>

      {/* Headline chart: correctness vs offered RPS (green, left axis) +
          corrected p99 latency (blue, right axis). One screenshot of the
          product's pitch — green cliffs while blue stays flat. */}
      <Card>
        <CardHeader
          title="Correctness vs load"
          subtitle={
            ticks.length > 0
              ? `${ticks.length} live tick(s) · pass=${latest?.pass_total} fail_status=${latest?.fail_status_total}`
              : 'Latency flat (blue, right axis) while correctness cliffs (green, left axis). Live via SSE.'
          }
        />
        <CardBody>
          <div className="flex justify-center">
            <HeadlineChart ticks={ticks} />
          </div>
        </CardBody>
      </Card>
    </div>
  )
}
