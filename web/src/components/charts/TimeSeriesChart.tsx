import { useMemo } from 'react'
import type { Tick } from '../../lib/api'

/**
 * Time-series view. X = elapsed_s. Three series on a shared X:
 *   - achieved RPS  (blue, left axis)
 *   - corrected p99 latency (purple, left axis but scaled to its own max)
 *   - per-tick pass rate (green, right axis: 0..100%)
 *
 * Reads worker desync (RPS oscillations vs target), ramp behavior, and the
 * latency-vs-correctness onset over time. Distinct from the cliff view, which
 * collapses time onto offered RPS.
 */
export function TimeSeriesChart({ ticks }: { ticks: Tick[] }) {
  const view = useMemo(() => buildView(ticks), [ticks])
  return (
    <div className="w-full">
      <ChartSVG view={view} />
      <Legend />
    </div>
  )
}

interface Point {
  t: number
  rps: number
  p99_ms: number
  pass_rate: number // 0..1
}

interface ViewModel {
  points: Point[]
  xMin: number
  xMax: number
  yRpsMax: number
  yLatMaxMs: number
}

function buildView(ticks: Tick[]): ViewModel {
  if (ticks.length === 0) {
    return { points: [], xMin: 0, xMax: 60, yRpsMax: 100, yLatMaxMs: 100 }
  }
  const points: Point[] = ticks.map((t) => ({
    t: t.elapsed_s,
    rps: t.achieved_rps_1s,
    p99_ms: (t.percentiles_so_far?.p99_us ?? 0) / 1000,
    pass_rate: t.this_tick.total > 0 ? t.this_tick.pass / t.this_tick.total : 1,
  }))
  const xMin = points[0].t
  const xMax = points[points.length - 1].t
  const yRpsMax = Math.max(10, Math.ceil(points.reduce((m, p) => Math.max(m, p.rps), 0) * 1.15))
  const yLatMaxMs = Math.max(10, Math.ceil(points.reduce((m, p) => Math.max(m, p.p99_ms), 0) * 1.15))
  return { points, xMin, xMax, yRpsMax, yLatMaxMs }
}

const W = 720
const H = 320
const PAD = { top: 20, right: 56, bottom: 36, left: 48 }
const INNER_W = W - PAD.left - PAD.right
const INNER_H = H - PAD.top - PAD.bottom

function ChartSVG({ view }: { view: ViewModel }) {
  const { points, xMin, xMax, yRpsMax, yLatMaxMs } = view
  if (points.length === 0) {
    return (
      <div
        className="flex items-center justify-center rounded-[var(--radius)] border border-border bg-surface-2/30 text-xs text-text-faint"
        style={{ width: W, height: H }}
      >
        No results to plot yet. This chart fills in as the run streams.
      </div>
    )
  }
  const xRange = Math.max(1, xMax - xMin)
  const xScale = (t: number) => PAD.left + ((t - xMin) / xRange) * INNER_W
  const yRPS = (rps: number) => PAD.top + (1 - rps / yRpsMax) * INNER_H
  const yLat = (ms: number) => PAD.top + (1 - ms / yLatMaxMs) * INNER_H
  const yPass = (rate: number) => PAD.top + (1 - rate) * INNER_H

  const path = (xs: Point[], yFn: (p: Point) => number) =>
    xs
      .map((p, i) => `${i === 0 ? 'M' : 'L'}${xScale(p.t).toFixed(1)},${yFn(p).toFixed(1)}`)
      .join(' ')

  // X ticks: every ~5 s, capped at 8 marks.
  const tickStep = Math.max(1, Math.ceil(xRange / 8))
  const xTicks: number[] = []
  for (let t = xMin; t <= xMax; t += tickStep) xTicks.push(t)

  return (
    <svg
      width={W}
      height={H}
      role="img"
      aria-label="Per-tick RPS, p99 latency, and pass rate over time"
      className="overflow-visible"
    >
      {/* Horizontal gridlines on pass-rate axis (0/25/50/75/100). */}
      {[0, 0.5, 1].map((r) => (
        <g key={r}>
          <line
            x1={PAD.left}
            x2={PAD.left + INNER_W}
            y1={yPass(r)}
            y2={yPass(r)}
            stroke="var(--color-border)"
            strokeWidth={1}
          />
          <text
            x={PAD.left + INNER_W + 8}
            y={yPass(r) + 4}
            className="fill-text-faint font-mono"
            style={{ fontSize: 10 }}
          >
            {Math.round(r * 100)}%
          </text>
        </g>
      ))}
      {/* Left axis: RPS labels (3 ticks). */}
      {[0, 0.5, 1].map((r) => (
        <text
          key={r}
          x={PAD.left - 8}
          y={yRPS(yRpsMax * r) + 4}
          textAnchor="end"
          className="fill-text-faint font-mono"
          style={{ fontSize: 10 }}
        >
          {Math.round(yRpsMax * r)}
        </text>
      ))}
      {/* X axis ticks. */}
      {xTicks.map((t) => (
        <g key={t}>
          <text
            x={xScale(t)}
            y={PAD.top + INNER_H + 16}
            textAnchor="middle"
            className="fill-text-faint font-mono"
            style={{ fontSize: 10 }}
          >
            {t}s
          </text>
        </g>
      ))}
      {/* Pass-rate (green, right axis). */}
      <path
        d={path(points, (p) => yPass(p.pass_rate))}
        fill="none"
        stroke="var(--color-correct)"
        strokeWidth={2}
        opacity={0.85}
      />
      {/* p99 latency (purple). yLatMaxMs scales independently of RPS so the
          two lines aren't squashed into the same axis when one dominates. */}
      <path
        d={path(points, (p) => yLat(p.p99_ms))}
        fill="none"
        stroke="var(--color-warn)"
        strokeWidth={2}
        opacity={0.85}
      />
      {/* Achieved RPS (blue). */}
      <path
        d={path(points, (p) => yRPS(p.rps))}
        fill="none"
        stroke="var(--color-latency)"
        strokeWidth={2}
        opacity={0.95}
      />
    </svg>
  )
}

function Legend() {
  return (
    <div className="mt-3 flex items-center justify-center gap-6 text-xs text-text-muted">
      <span className="inline-flex items-center gap-2">
        <span className="inline-block h-0.5 w-6 bg-latency" />
        Achieved RPS
      </span>
      <span className="inline-flex items-center gap-2">
        <span className="inline-block h-0.5 w-6 bg-warn" />
        p99 latency (ms)
      </span>
      <span className="inline-flex items-center gap-2">
        <span className="inline-block h-0.5 w-6 bg-correct" />
        Pass rate
      </span>
    </div>
  )
}
