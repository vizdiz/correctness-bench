import { useMemo } from 'react'
import type { Tick } from '../../lib/api'

/**
 * The headline chart: correctness % (green, left axis) and corrected p99
 * latency (blue, right axis) on a shared offered-RPS X-axis. Each tick is
 * one data point, bucketed onto its `buckets[0].rps_lo`. If the same bucket
 * appears in multiple ticks, correctness counts sum; latency is the latest.
 *
 * The shape this draws — the GREEN line dropping while the BLUE line stays
 * flat — IS the product's pitch.
 */
export function HeadlineChart({ ticks }: { ticks: Tick[] }) {
  const view = useMemo(() => buildView(ticks), [ticks])
  return (
    <div className="w-full">
      <ChartSVG view={view} />
      <Legend />
    </div>
  )
}

interface BucketPoint {
  rps_lo: number
  rps_hi: number
  rps_mid: number
  total: number
  pass: number
  pass_rate: number // 0..1
  p99_us: number // latest in this bucket
}

interface ViewModel {
  points: BucketPoint[]
  xMin: number
  xMax: number
  yLatMax_us: number
}

function buildView(ticks: Tick[]): ViewModel {
  if (ticks.length === 0) {
    return { points: [], xMin: 0, xMax: 100, yLatMax_us: 100_000 }
  }
  const byBucket = new Map<number, BucketPoint>()
  for (const t of ticks) {
    const b = t.buckets?.[0]
    if (!b || b.total === 0) continue
    const cur = byBucket.get(b.rps_lo)
    const p99 = t.percentiles_so_far?.p99_us ?? 0
    if (cur) {
      cur.total += b.total
      cur.pass += b.pass
      cur.pass_rate = cur.total > 0 ? cur.pass / cur.total : 1
      cur.p99_us = p99 // latest sample wins
    } else {
      byBucket.set(b.rps_lo, {
        rps_lo: b.rps_lo,
        rps_hi: b.rps_hi,
        rps_mid: (b.rps_lo + b.rps_hi) / 2,
        total: b.total,
        pass: b.pass,
        pass_rate: b.total > 0 ? b.pass / b.total : 1,
        p99_us: p99,
      })
    }
  }
  const points = Array.from(byBucket.values()).sort((a, b) => a.rps_lo - b.rps_lo)
  const xMin = points.length > 0 ? points[0].rps_lo : 0
  const xMax = points.length > 0 ? points[points.length - 1].rps_hi : 100
  // Round latency axis up to a multiple of 5 ms for sane gridlines.
  const maxP99 = points.reduce((m, p) => Math.max(m, p.p99_us), 0)
  const yLatMax_us = Math.max(50_000, Math.ceil((maxP99 * 1.25) / 5000) * 5000)
  return { points, xMin, xMax, yLatMax_us }
}

const W = 720
const H = 320
const PAD = { top: 20, right: 56, bottom: 36, left: 48 }
const INNER_W = W - PAD.left - PAD.right
const INNER_H = H - PAD.top - PAD.bottom

function ChartSVG({ view }: { view: ViewModel }) {
  const { points, xMin, xMax, yLatMax_us } = view
  if (points.length === 0) {
    return (
      <div
        className="flex items-center justify-center rounded-[var(--radius)] border border-border bg-surface-2/30 text-xs text-text-faint"
        style={{ width: W, height: H }}
      >
        No results to plot yet — this chart fills in as the run streams.
      </div>
    )
  }

  const xRange = Math.max(1, xMax - xMin)
  const xScale = (rps: number) =>
    PAD.left + ((rps - xMin) / xRange) * INNER_W
  const yCorrect = (rate: number) =>
    PAD.top + (1 - rate) * INNER_H // 0..1 → top..bottom (1 at top)
  const yLat = (us: number) =>
    PAD.top + (1 - us / yLatMax_us) * INNER_H

  const correctnessPath = points
    .map((p, i) => `${i === 0 ? 'M' : 'L'}${xScale(p.rps_mid).toFixed(1)},${yCorrect(p.pass_rate).toFixed(1)}`)
    .join(' ')
  const latencyPath = points
    .map((p, i) => `${i === 0 ? 'M' : 'L'}${xScale(p.rps_mid).toFixed(1)},${yLat(p.p99_us).toFixed(1)}`)
    .join(' ')

  // Gridlines: 5 horizontal on the left scale (0/25/50/75/100%).
  const gridRows = [0, 0.25, 0.5, 0.75, 1].map((r) => ({
    y: yCorrect(r),
    label: `${Math.round(r * 100)}%`,
  }))
  // X ticks: ~6 across the rps range.
  const xTickCount = Math.min(6, points.length)
  const xTicks: number[] = []
  for (let i = 0; i < xTickCount; i++) {
    xTicks.push(xMin + (xRange * i) / (xTickCount - 1 || 1))
  }
  // Right-axis labels: convert yLatMax_us into ms steps (50/25/0% of axis).
  const latLabels = [0, 0.5, 1].map((r) => ({
    y: PAD.top + (1 - r) * INNER_H,
    label: `${((yLatMax_us * r) / 1000).toFixed(0)} ms`,
  }))

  return (
    <svg
      width={W}
      height={H}
      role="img"
      aria-label="Correctness vs offered RPS, with corrected p99 latency on the right axis"
      className="overflow-visible"
    >
      {/* Background */}
      <rect
        x={PAD.left}
        y={PAD.top}
        width={INNER_W}
        height={INNER_H}
        fill="var(--color-surface-2)"
        rx="6"
        opacity="0.45"
      />
      {/* Horizontal gridlines on the correctness scale */}
      {gridRows.map((g, i) => (
        <g key={i}>
          <line
            x1={PAD.left}
            x2={PAD.left + INNER_W}
            y1={g.y}
            y2={g.y}
            stroke="var(--color-border)"
            strokeWidth={1}
            strokeDasharray={i === 0 || i === gridRows.length - 1 ? '' : '2 4'}
          />
          <text
            x={PAD.left - 8}
            y={g.y + 4}
            textAnchor="end"
            className="fill-text-faint font-mono"
            style={{ fontSize: 10 }}
          >
            {g.label}
          </text>
        </g>
      ))}
      {/* X-axis ticks + labels */}
      {xTicks.map((rps, i) => (
        <g key={i}>
          <line
            x1={xScale(rps)}
            x2={xScale(rps)}
            y1={PAD.top + INNER_H}
            y2={PAD.top + INNER_H + 4}
            stroke="var(--color-border-strong)"
            strokeWidth={1}
          />
          <text
            x={xScale(rps)}
            y={PAD.top + INNER_H + 16}
            textAnchor="middle"
            className="fill-text-faint font-mono"
            style={{ fontSize: 10 }}
          >
            {Math.round(rps)}
          </text>
        </g>
      ))}
      {/* Right-axis labels (latency) */}
      {latLabels.map((l, i) => (
        <text
          key={i}
          x={PAD.left + INNER_W + 8}
          y={l.y + 4}
          className="fill-text-faint font-mono"
          style={{ fontSize: 10 }}
        >
          {l.label}
        </text>
      ))}
      {/* Latency line (blue) — drawn under the correctness line. */}
      <path d={latencyPath} fill="none" stroke="var(--color-latency)" strokeWidth={2} opacity={0.85} />
      {/* Correctness line (green) — the headline. */}
      <path
        d={correctnessPath}
        fill="none"
        stroke="var(--color-correct)"
        strokeWidth={2.5}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      {/* Marker dots on correctness for each measured bucket */}
      {points.map((p, i) => (
        <circle
          key={i}
          cx={xScale(p.rps_mid)}
          cy={yCorrect(p.pass_rate)}
          r={2.5}
          fill="var(--color-correct)"
        />
      ))}
      {/* Axis title — bottom (X) */}
      <text
        x={PAD.left + INNER_W / 2}
        y={H - 6}
        textAnchor="middle"
        className="fill-text-muted"
        style={{ fontSize: 11 }}
      >
        Offered RPS (bucket center)
      </text>
    </svg>
  )
}

function Legend() {
  return (
    <div className="mt-3 flex items-center justify-center gap-6 text-xs text-text-muted">
      <span className="inline-flex items-center gap-2">
        <span className="inline-block h-0.5 w-6 bg-correct" />
        Correctness (left)
      </span>
      <span className="inline-flex items-center gap-2">
        <span className="inline-block h-0.5 w-6 bg-latency" />
        Latency p99 (right)
      </span>
    </div>
  )
}
