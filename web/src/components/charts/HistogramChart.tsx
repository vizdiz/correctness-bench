import { useEffect, useMemo, useState } from 'react'
import { api, type FinalsView, type HistogramJSON, type Tick } from '../../lib/api'

/**
 * Histogram view. When a run is finalized, fetch the binned distribution from
 * the server (GET /v1/runs/:id/histogram?format=json) - that's the real HDR.
 * While the run is live, fall back to sampling per-tick p99 values. Vertical
 * marker lines show the run-final p50/p95/p99 when present.
 *
 * The shape reveals tail stability: tight cluster vs long right tail tells a
 * different operational story than the median alone.
 */
export function HistogramChart({
  runId,
  ticks,
  finals,
}: {
  runId?: string
  ticks: Tick[]
  finals?: FinalsView
}) {
  const [server, setServer] = useState<HistogramJSON | null>(null)
  const [unavailable, setUnavailable] = useState(false)

  useEffect(() => {
    if (!runId || !finals) {
      setServer(null)
      setUnavailable(false)
      return
    }
    let cancelled = false
    api
      .histogramJSON(runId, 'corrected')
      .then((h) => {
        if (!cancelled) setServer(h)
      })
      .catch(() => {
        if (!cancelled) setUnavailable(true)
      })
    return () => {
      cancelled = true
    }
  }, [runId, finals])

  const view = useMemo(() => {
    if (server) return buildViewFromServer(server)
    return buildViewFromTicks(ticks, finals)
  }, [server, ticks, finals])

  return (
    <div className="w-full">
      <ChartSVG view={view} />
      <Legend source={server ? 'server' : 'sampled'} unavailable={unavailable && !server} />
    </div>
  )
}

interface Bin {
  loMs: number
  hiMs: number
  count: number
}

interface ViewModel {
  bins: Bin[]
  maxCount: number
  axisLoMs: number
  axisHiMs: number
  markers: { ms: number; label: string }[]
  totalSamples: number
}

const BIN_COUNT = 24

function buildViewFromServer(h: HistogramJSON): ViewModel {
  const bins: Bin[] = h.bins.map((b) => ({
    loMs: b.lo_us / 1000,
    hiMs: b.hi_us / 1000,
    count: b.count,
  }))
  const maxCount = bins.reduce((m, b) => Math.max(m, b.count), 1)
  const markers = [
    { ms: h.p50_us / 1000, label: 'p50' },
    { ms: h.p95_us / 1000, label: 'p95' },
    { ms: h.p99_us / 1000, label: 'p99' },
  ]
  return {
    bins,
    maxCount,
    axisLoMs: (h.min_us || h.bins[0]?.lo_us || 0) / 1000,
    axisHiMs: (h.max_us || h.bins[h.bins.length - 1]?.hi_us || 1) / 1000,
    markers,
    totalSamples: h.total,
  }
}

function buildViewFromTicks(ticks: Tick[], finals?: FinalsView): ViewModel {
  const samples = ticks
    .map((t) => t.percentiles_so_far?.p99_us ?? 0)
    .filter((us) => us > 0)
    .map((us) => us / 1000) // to ms

  if (samples.length === 0) {
    return {
      bins: [],
      maxCount: 0,
      axisLoMs: 0,
      axisHiMs: 100,
      markers: [],
      totalSamples: 0,
    }
  }

  // Bound the axis on the data so the bins fill the space.
  const lo = Math.min(...samples)
  const hi = Math.max(...samples)
  const span = Math.max(1, hi - lo)
  const axisLoMs = Math.max(0, Math.floor(lo - span * 0.05))
  const axisHiMs = Math.ceil(hi + span * 0.05)
  const binSpan = (axisHiMs - axisLoMs) / BIN_COUNT

  const bins: Bin[] = Array.from({ length: BIN_COUNT }, (_, i) => ({
    loMs: axisLoMs + i * binSpan,
    hiMs: axisLoMs + (i + 1) * binSpan,
    count: 0,
  }))
  for (const ms of samples) {
    let idx = Math.floor((ms - axisLoMs) / binSpan)
    if (idx >= BIN_COUNT) idx = BIN_COUNT - 1
    if (idx < 0) idx = 0
    bins[idx].count++
  }
  const maxCount = bins.reduce((m, b) => Math.max(m, b.count), 1)

  const markers: { ms: number; label: string }[] = []
  if (finals?.corrected) {
    markers.push(
      { ms: finals.corrected.p50_us / 1000, label: 'p50' },
      { ms: finals.corrected.p95_us / 1000, label: 'p95' },
      { ms: finals.corrected.p99_us / 1000, label: 'p99' },
    )
  }

  return { bins, maxCount, axisLoMs, axisHiMs, markers, totalSamples: samples.length }
}

const W = 720
const H = 280
const PAD = { top: 16, right: 24, bottom: 36, left: 44 }
const INNER_W = W - PAD.left - PAD.right
const INNER_H = H - PAD.top - PAD.bottom

function ChartSVG({ view }: { view: ViewModel }) {
  const { bins, maxCount, axisLoMs, axisHiMs, markers, totalSamples } = view
  if (totalSamples === 0) {
    return (
      <div
        className="flex items-center justify-center rounded-[var(--radius)] border border-border bg-surface-2/30 text-xs text-text-faint"
        style={{ width: W, height: H }}
      >
        No results to plot yet — this chart fills in as the run streams.
      </div>
    )
  }

  const xRange = Math.max(1, axisHiMs - axisLoMs)
  const xScale = (ms: number) => PAD.left + ((ms - axisLoMs) / xRange) * INNER_W
  const yScale = (count: number) => PAD.top + (1 - count / maxCount) * INNER_H

  const xTickCount = 6
  const xTicks: number[] = []
  for (let i = 0; i < xTickCount; i++) {
    xTicks.push(axisLoMs + (xRange * i) / (xTickCount - 1))
  }
  const yTickCount = 4
  const yTicks: number[] = []
  for (let i = 0; i <= yTickCount; i++) {
    yTicks.push(Math.round((maxCount * i) / yTickCount))
  }

  return (
    <svg
      width={W}
      height={H}
      role="img"
      aria-label="Histogram of per-tick p99 latency"
      className="overflow-visible"
    >
      {/* Y gridlines + labels */}
      {yTicks.map((y, i) => (
        <g key={i}>
          <line
            x1={PAD.left}
            x2={PAD.left + INNER_W}
            y1={yScale(y)}
            y2={yScale(y)}
            stroke="var(--color-border)"
            strokeWidth={1}
          />
          <text
            x={PAD.left - 8}
            y={yScale(y) + 4}
            textAnchor="end"
            className="fill-text-faint font-mono"
            style={{ fontSize: 10 }}
          >
            {y}
          </text>
        </g>
      ))}
      {/* X labels */}
      {xTicks.map((ms, i) => (
        <text
          key={i}
          x={xScale(ms)}
          y={PAD.top + INNER_H + 16}
          textAnchor="middle"
          className="fill-text-faint font-mono"
          style={{ fontSize: 10 }}
        >
          {ms.toFixed(0)}
        </text>
      ))}
      {/* Bars */}
      {bins.map((b, i) => {
        const x = xScale(b.loMs)
        const wpx = Math.max(1, xScale(b.hiMs) - xScale(b.loMs) - 1)
        const y = yScale(b.count)
        const h = PAD.top + INNER_H - y
        return (
          <rect
            key={i}
            x={x}
            y={y}
            width={wpx}
            height={h}
            fill="var(--color-latency)"
            opacity={b.count > 0 ? 0.75 : 0}
          />
        )
      })}
      {/* Final percentile markers */}
      {markers.map((m, i) => (
        <g key={i}>
          <line
            x1={xScale(m.ms)}
            x2={xScale(m.ms)}
            y1={PAD.top}
            y2={PAD.top + INNER_H}
            stroke="var(--color-correct)"
            strokeWidth={1.5}
            strokeDasharray="3 3"
          />
          <text
            x={xScale(m.ms)}
            y={PAD.top - 4}
            textAnchor="middle"
            className="fill-correct font-mono"
            style={{ fontSize: 10 }}
          >
            {m.label}
          </text>
        </g>
      ))}
    </svg>
  )
}

function Legend({
  source,
  unavailable,
}: {
  source: 'server' | 'sampled'
  unavailable: boolean
}) {
  return (
    <div className="mt-3 flex items-center justify-center gap-6 text-xs text-text-muted">
      <span className="inline-flex items-center gap-2">
        <span className="inline-block h-3 w-3 bg-latency opacity-75" />
        {source === 'server'
          ? 'HDR distribution (server-binned)'
          : 'Per-tick p99 sampled'}
      </span>
      <span className="inline-flex items-center gap-2">
        <span className="inline-block h-0.5 w-6 border-t border-dashed border-correct" />
        Run-final percentile
      </span>
      {unavailable && (
        <span className="text-warn">HDR not available yet — using samples</span>
      )}
    </div>
  )
}
