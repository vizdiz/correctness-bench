// Number formatting. Rule (web agent brief): round EVERY displayed number.

/** Microseconds -> milliseconds, rounded to 1 decimal. */
export function usToMs(us: number): string {
  return (us / 1000).toFixed(1)
}

/** Round to a sane number of significant places for a dashboard. */
export function round(n: number, places = 1): string {
  return n.toFixed(places)
}

/** 0..1 ratio -> integer percent. */
export function pct(ratio: number): string {
  return `${Math.round(ratio * 100)}%`
}

/** Compact integer with thousands separators. */
export function int(n: number): string {
  return Math.round(n).toLocaleString('en-US')
}

/** ISO timestamp -> short local-ish display. */
export function shortTime(iso?: string | null): string {
  if (!iso) return '—'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}
