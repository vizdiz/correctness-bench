import { useEffect, useRef, useState } from 'react'
import { useParams, Link } from 'react-router-dom'
import { api, type RunView, type Tick } from '../lib/api'
import { Button } from '../components/ui/Button'
import { Card, CardBody, CardHeader } from '../components/ui/Card'
import { NumberDisplay } from '../components/ui/NumberDisplay'
import { StatusBadge } from '../components/ui/StatusBadge'
import { HeadlineChart } from '../components/charts/HeadlineChart'
import { HistogramChart } from '../components/charts/HistogramChart'
import { TimeSeriesChart } from '../components/charts/TimeSeriesChart'
import { int, round, usToMs } from '../lib/format'

const terminal = new Set(['completed', 'failed', 'aborted'])
const MAX_TICKS_HISTORY = 120

type ChartTab = 'headline' | 'histogram' | 'time-series'

export function RunDetail() {
  const [chartTab, setChartTab] = useState<ChartTab>('headline')
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

  // Seed from persisted history so a completed run renders its charts (live SSE
  // only carries ticks while the run is streaming). Live ticks append on top,
  // deduped by elapsed_s so a running run doesn't double-count.
  useEffect(() => {
    if (!id) return
    api
      .getTicks(id)
      .then((history) => {
        if (history.length === 0) return
        setTicks((prev) => {
          const seen = new Set(prev.map((t) => t.elapsed_s))
          const merged = [...history.filter((t) => !seen.has(t.elapsed_s)), ...prev]
          return merged.sort((a, b) => a.elapsed_s - b.elapsed_s).slice(-MAX_TICKS_HISTORY)
        })
      })
      .catch(() => {
        /* history is best-effort — live SSE still works */
      })
  }, [id])

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
          if (prev.some((t) => t.elapsed_s === tick.elapsed_s)) return prev
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
              title={streamLive ? 'Live updates connected' : 'Live updates paused'}
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

      {/* Tabbed chart panel. Headline is the pitch (cliff + flat latency).
          Histogram is tail-stability. Time-series is worker desync + ramp. */}
      <Card>
        <CardHeader
          title={
            chartTab === 'headline'
              ? 'Correctness vs load'
              : chartTab === 'histogram'
              ? 'Latency distribution'
              : 'Per-tick over time'
          }
          subtitle={
            ticks.length > 0
              ? `${ticks.length} data point(s) · ${latest?.pass_total} passed · ${latest?.fail_status_total} wrong-status`
              : 'No results yet — this updates live as the run streams.'
          }
          actions={
            <div className="inline-flex rounded-[var(--radius)] border border-border bg-surface-2/30 p-0.5 text-xs">
              {(
                [
                  ['headline', 'Cliff'],
                  ['histogram', 'Histogram'],
                  ['time-series', 'Time'],
                ] as [ChartTab, string][]
              ).map(([k, label]) => (
                <button
                  key={k}
                  onClick={() => setChartTab(k)}
                  className={`rounded-[calc(var(--radius)-2px)] px-2.5 py-1 transition-colors ${
                    chartTab === k
                      ? 'bg-accent/15 text-accent'
                      : 'text-text-faint hover:text-text-muted'
                  }`}
                >
                  {label}
                </button>
              ))}
            </div>
          }
        />
        <CardBody>
          <div className="flex justify-center">
            {chartTab === 'headline' && <HeadlineChart ticks={ticks} />}
            {chartTab === 'histogram' && (
              <HistogramChart runId={id} ticks={ticks} finals={run.final} />
            )}
            {chartTab === 'time-series' && <TimeSeriesChart ticks={ticks} />}
          </div>
        </CardBody>
      </Card>
    </div>
  )
}
