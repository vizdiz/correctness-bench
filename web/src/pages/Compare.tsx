import { useEffect, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { api, type CompareResponse, type RegressionResponse, type RunView } from '../lib/api'
import { Button } from '../components/ui/Button'
import { Card, CardBody, CardHeader } from '../components/ui/Card'
import { NumberDisplay } from '../components/ui/NumberDisplay'
import { StatusBadge } from '../components/ui/StatusBadge'
import { int, round, usToMs } from '../lib/format'

type Axis = keyof CompareResponse['winner_by']

const AXES: { key: Axis; label: string; help: string }[] = [
  { key: 'latency_p99', label: 'p99 latency', help: 'lower is better' },
  { key: 'correctness', label: 'correctness', help: 'higher pass rate' },
  { key: 'cost_per_request', label: 'cost / request', help: 'lower is better' },
  { key: 'tail_stability', label: 'tail stability', help: 'smaller p99 - p50 spread' },
  { key: 'rate_limit_onset', label: 'cliff RPS', help: 'higher onset is better' },
]

export function Compare() {
  const { idA = '', idB = '' } = useParams()
  const [data, setData] = useState<CompareResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [regression, setRegression] = useState<RegressionResponse | null>(null)
  const [regError, setRegError] = useState<string | null>(null)
  const [running, setRunning] = useState(false)
  // A is the candidate, B is the baseline. Switching is one click.
  const [thresholds, setThresholds] = useState({ p99: 10, correctness: 1 })

  useEffect(() => {
    setError(null)
    api.compare(idA, idB).then(setData).catch((e) => setError(e.message))
  }, [idA, idB])

  async function runRegression() {
    setRegError(null)
    setRunning(true)
    try {
      const r = await api.regressionCheck(idA, idB, {
        p99_delta_pct: thresholds.p99,
        correctness_delta_pct: thresholds.correctness,
      })
      setRegression(r)
    } catch (e) {
      setRegError((e as Error).message)
    } finally {
      setRunning(false)
    }
  }

  if (error) {
    return (
      <Card className="border-danger/40 bg-danger/5 px-5 py-4 text-sm text-danger">
        {error} — <Link to="/runs" className="underline">back to runs</Link>
      </Card>
    )
  }
  if (!data) return <p className="text-sm text-text-faint">Loading comparison…</p>

  const f = data.fairness
  const unfair = !f.same_load_shape || !f.same_assert_spec
  return (
    <div>
      <header className="mb-6">
        <h1 className="text-xl font-semibold tracking-tight">A vs B comparison</h1>
        <p className="mt-1 text-xs text-text-faint">
          <Link className="underline" to={`/runs/${idA}`}>{idA}</Link>
          {' '}vs{' '}
          <Link className="underline" to={`/runs/${idB}`}>{idB}</Link>
        </p>
      </header>

      {unfair && (
        <Card className="mb-6 border-warn/40 bg-warn/5 px-5 py-3 text-sm text-warn">
          Fairness flagged — {!f.same_load_shape && 'load shapes differ (target_rps / duration_s)'}
          {!f.same_load_shape && !f.same_assert_spec && '; '}
          {!f.same_assert_spec && 'assert specs differ'}. Treat winner_by as advisory.
        </Card>
      )}

      <div className="mb-6 grid grid-cols-1 gap-4 md:grid-cols-2">
        <RunCard label="A" view={data.a} />
        <RunCard label="B" view={data.b} />
      </div>

      <Card className="mb-6">
        <CardHeader
          title="Regression check"
          subtitle="A is candidate, B is baseline. Thresholds default to api.md's example (p99 +10%, correctness -1pp)."
          actions={
            <Button onClick={runRegression} disabled={running}>
              {running ? 'Checking…' : 'Run check'}
            </Button>
          }
        />
        <CardBody className="space-y-3">
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
            <ThresholdInput
              label="p99 ceiling"
              value={thresholds.p99}
              onChange={(n) => setThresholds((t) => ({ ...t, p99: n }))}
              suffix="%"
            />
            <ThresholdInput
              label="correctness floor"
              value={thresholds.correctness}
              onChange={(n) => setThresholds((t) => ({ ...t, correctness: n }))}
              suffix="pp"
            />
          </div>
          {regError && (
            <p className="text-sm text-danger">{regError}</p>
          )}
          {regression && (
            <div
              className={`rounded-[var(--radius)] border px-4 py-3 text-sm ${
                regression.passed
                  ? 'border-correct/40 bg-correct/5 text-correct'
                  : 'border-danger/40 bg-danger/5 text-danger'
              }`}
            >
              <div className="flex items-center justify-between">
                <span className="font-semibold">
                  {regression.passed ? 'PASS' : 'FAIL'}
                </span>
                <span className="font-mono text-xs">
                  p99 {regression.p99_delta_pct.toFixed(2)}% ·
                  correctness {regression.correctness_delta_pct.toFixed(2)}pp ·
                  cliff {regression.cliff_rps_delta.toFixed(1)} RPS
                </span>
              </div>
              <p className="mt-1">{regression.details}</p>
            </div>
          )}
        </CardBody>
      </Card>

      <Card>
        <CardHeader title="Winner by axis" />
        <CardBody>
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left text-xs uppercase tracking-wide text-text-faint">
                <th className="pb-2">Axis</th>
                <th className="pb-2">A</th>
                <th className="pb-2">B</th>
                <th className="pb-2">Winner</th>
              </tr>
            </thead>
            <tbody>
              {AXES.map((a) => {
                const winner = data.winner_by[a.key]
                const aVal = axisValue(a.key, data.a)
                const bVal = axisValue(a.key, data.b)
                return (
                  <tr key={a.key} className="border-t border-border">
                    <td className="py-2 align-top">
                      <div className="font-medium text-text">{a.label}</div>
                      <div className="text-xs text-text-faint">{a.help}</div>
                    </td>
                    <td className="py-2 font-mono text-text">{aVal}</td>
                    <td className="py-2 font-mono text-text">{bVal}</td>
                    <td className="py-2">
                      {winner === 'a' && <Badge tone="correct">A</Badge>}
                      {winner === 'b' && <Badge tone="correct">B</Badge>}
                      {!winner && <span className="text-text-faint">—</span>}
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </CardBody>
      </Card>
    </div>
  )
}

function RunCard({ label, view }: { label: 'A' | 'B'; view: RunView }) {
  return (
    <Card>
      <CardHeader
        title={
          <span className="inline-flex items-center gap-2">
            <span className="inline-flex h-5 w-5 items-center justify-center rounded-full bg-accent/15 text-xs font-semibold text-accent">
              {label}
            </span>
            {view.name || <span className="text-text-faint">untitled</span>}
            <StatusBadge status={view.status} />
          </span>
        }
        subtitle={
          <span className="font-mono text-xs">
            <Link className="underline" to={`/runs/${view.run_id}`}>{view.run_id}</Link>
          </span>
        }
      />
      <CardBody className="grid grid-cols-2 gap-4">
        <NumberDisplay label="Target RPS" value={int(view.target_rps)} tone="accent" />
        <NumberDisplay label="Duration" value={view.duration_s != null ? String(view.duration_s) : '—'} unit="s" tone="accent" />
        <NumberDisplay
          label="Correctness"
          value={view.final ? round(view.final.correctness_pct) + '%' : '—'}
          tone={view.final && view.final.correctness_pct < 95 ? 'danger' : 'correct'}
        />
        <NumberDisplay
          label="p99"
          value={view.final ? usToMs(view.final.corrected.p99_us) : '—'}
          unit="ms"
          tone="accent"
        />
        <NumberDisplay
          label="p50"
          value={view.final ? usToMs(view.final.corrected.p50_us) : '—'}
          unit="ms"
          tone="accent"
        />
        <NumberDisplay
          label="Requests"
          value={view.final ? int(view.final.total_requests) : '—'}
          tone="accent"
        />
      </CardBody>
    </Card>
  )
}

function axisValue(axis: Axis, view: RunView): string {
  if (!view.final) return '—'
  switch (axis) {
    case 'latency_p99':
      return usToMs(view.final.corrected.p99_us) + ' ms'
    case 'correctness':
      return round(view.final.correctness_pct) + '%'
    case 'cost_per_request':
      if (view.cost_per_request_usd == null) return '—'
      return '$' + view.cost_per_request_usd.toFixed(6)
    case 'tail_stability': {
      const spread = view.final.corrected.p99_us - view.final.corrected.p50_us
      return usToMs(spread) + ' ms'
    }
    case 'rate_limit_onset':
      return view.final.cliff_rps != null ? round(view.final.cliff_rps) + ' RPS' : '—'
  }
}

function ThresholdInput({
  label,
  value,
  onChange,
  suffix,
}: {
  label: string
  value: number
  onChange: (n: number) => void
  suffix: string
}) {
  return (
    <label className="block">
      <span className="block text-xs uppercase tracking-wide text-text-faint">{label}</span>
      <span className="mt-1 inline-flex items-center gap-1">
        <input
          type="number"
          inputMode="decimal"
          step={0.1}
          min={0}
          value={value}
          onChange={(e) => onChange(Number(e.target.value) || 0)}
          className="w-20 rounded-md border border-border bg-surface-2/30 px-2 py-1 text-right font-mono text-sm"
        />
        <span className="text-xs text-text-faint">{suffix}</span>
      </span>
    </label>
  )
}

function Badge({ tone, children }: { tone: 'correct'; children: React.ReactNode }) {
  const cls =
    tone === 'correct'
      ? 'border-correct/40 bg-correct/10 text-correct'
      : 'border-border text-text-faint'
  return (
    <span className={`inline-flex items-center rounded-full border px-2 py-0.5 text-xs font-medium ${cls}`}>
      {children}
    </span>
  )
}
