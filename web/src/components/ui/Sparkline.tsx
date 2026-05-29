import { cn } from '../../lib/cn'

/**
 * Sparkline PLACEHOLDER. Renders a tiny inline SVG polyline from a series so the
 * layout and color language are pinned now; the real charts (headline cliff,
 * histogram, time-series) replace this once SSE data flows. If no data is given,
 * draws a flat baseline so empty states still look intentional.
 */
export function Sparkline({
  data,
  width = 120,
  height = 28,
  className,
  tone = 'accent',
}: {
  data?: number[]
  width?: number
  height?: number
  className?: string
  tone?: 'accent' | 'correct' | 'danger'
}) {
  const series = data && data.length > 1 ? data : [0.5, 0.5]
  const min = Math.min(...series)
  const max = Math.max(...series)
  const span = max - min || 1
  const stepX = width / (series.length - 1)
  const points = series
    .map((v, i) => {
      const x = i * stepX
      const y = height - ((v - min) / span) * (height - 4) - 2
      return `${x.toFixed(1)},${y.toFixed(1)}`
    })
    .join(' ')

  const stroke =
    tone === 'correct'
      ? 'var(--color-correct)'
      : tone === 'danger'
        ? 'var(--color-danger)'
        : 'var(--color-accent)'

  return (
    <svg
      width={width}
      height={height}
      className={cn('overflow-visible', className)}
      role="img"
      aria-label="sparkline placeholder"
    >
      <polyline
        points={points}
        fill="none"
        stroke={stroke}
        strokeWidth={1.5}
        strokeLinecap="round"
        strokeLinejoin="round"
        opacity={data ? 1 : 0.35}
      />
    </svg>
  )
}
