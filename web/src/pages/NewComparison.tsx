import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api } from '../lib/api'
import { Button } from '../components/ui/Button'
import { Card, CardBody } from '../components/ui/Card'
import { Field, Input } from '../components/ui/Input'

/**
 * Fire two targets against one shared schedule window (concurrent A/B). The
 * comparison is fair: same time, same conditions, each target at the full RPS.
 */
export function NewComparison() {
  const nav = useNavigate()
  const [name, setName] = useState('')
  const [urlA, setUrlA] = useState('')
  const [urlB, setUrlB] = useState('')
  const [rps, setRps] = useState('200')
  const [duration, setDuration] = useState('30')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      const res = await api.createComparison({
        name: name || undefined,
        target_a: { url: urlA, method: 'GET' },
        target_b: { url: urlB, method: 'GET' },
        target_rps: Number(rps),
        duration_s: Number(duration),
        connections: 50,
        keepalive: true,
        load_model: 'open',
        assert: { expected_status: [200] },
      })
      nav(`/compare/${res.run_a}/${res.run_b}`)
    } catch (err) {
      setError((err as Error).message)
      setBusy(false)
    }
  }

  return (
    <div>
      <header className="mb-6">
        <h1 className="text-xl font-semibold tracking-tight">Compare two APIs</h1>
        <p className="mt-1 max-w-xl text-sm text-text-muted">
          Both targets run the same load at the same time, so the result is fair:
          which one stays correct under load, at what latency and cost.
        </p>
      </header>

      <Card>
        <CardBody>
          <form onSubmit={submit} className="flex flex-col gap-4">
            <Field label="Name (optional)">
              <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="vendorA vs vendorB" />
            </Field>
            <Field label="Target A URL">
              <Input required value={urlA} onChange={(e) => setUrlA(e.target.value)} placeholder="https://api.vendor-a.com/v1/..." />
            </Field>
            <Field label="Target B URL">
              <Input required value={urlB} onChange={(e) => setUrlB(e.target.value)} placeholder="https://api.vendor-b.com/v1/..." />
            </Field>
            <div className="flex gap-4">
              <Field label="Target RPS (each)">
                <Input type="number" value={rps} onChange={(e) => setRps(e.target.value)} />
              </Field>
              <Field label="Duration (s)">
                <Input type="number" value={duration} onChange={(e) => setDuration(e.target.value)} />
              </Field>
            </div>
            {error && <p className="text-sm text-danger">{error}</p>}
            <div>
              <Button type="submit" disabled={busy}>
                {busy ? 'Starting…' : 'Run comparison'}
              </Button>
            </div>
          </form>
        </CardBody>
      </Card>
    </div>
  )
}
