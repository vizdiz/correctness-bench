import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api, ApiError, type CreateRunRequest } from '../lib/api'
import { Button } from '../components/ui/Button'
import { Card, CardBody, CardHeader } from '../components/ui/Card'
import { Field, Input, Select } from '../components/ui/Input'

export function RunNew() {
  const nav = useNavigate()
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<{ message: string; field?: string } | null>(null)

  async function onSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault()
    setError(null)
    const f = new FormData(e.currentTarget)

    const apiKey = String(f.get('api_key') || '').trim()
    const headers: Record<string, string> = { 'Content-Type': 'application/json' }
    if (apiKey) headers['Authorization'] = `Bearer ${apiKey}`

    const body: CreateRunRequest = {
      name: String(f.get('name') || '') || undefined,
      target: {
        url: String(f.get('url') || ''),
        method: String(f.get('method') || 'GET'),
        headers,
      },
      target_rps: Number(f.get('target_rps')),
      duration_s: Number(f.get('duration_s')),
      load_model: String(f.get('load_model') || 'open') as 'open' | 'closed',
      connections: Number(f.get('connections')),
      keepalive: true,
      assert: { expected_status: [200] },
      rate_limit_policy: { action: 'backoff', record_onset: true },
    }

    setSubmitting(true)
    try {
      const res = await api.createRun(body)
      nav(`/runs/${res.run_id}`)
    } catch (err) {
      if (err instanceof ApiError) setError({ message: err.message, field: err.field })
      else setError({ message: (err as Error).message })
      setSubmitting(false)
    }
  }

  return (
    <div className="mx-auto max-w-2xl">
      <header className="mb-6">
        <h1 className="text-xl font-semibold tracking-tight">New run</h1>
        <p className="mt-1 text-sm text-text-muted">
          Define the target and load shape. Engine wiring lands next — for now this
          queues a run through the control plane.
        </p>
      </header>

      <Card>
        <CardHeader title="Target & load" />
        <CardBody>
          <form onSubmit={onSubmit} className="flex flex-col gap-4">
            <Field label="Name" htmlFor="name">
              <Input id="name" name="name" placeholder="openai vs anthropic — 100rps" />
            </Field>

            <div className="grid grid-cols-[1fr_auto] gap-3">
              <Field label="Target URL" htmlFor="url">
                <Input id="url" name="url" required placeholder="https://api.example.com/v1/foo" />
              </Field>
              <Field label="Method" htmlFor="method">
                <Select id="method" name="method" defaultValue="POST">
                  {['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD'].map((m) => (
                    <option key={m}>{m}</option>
                  ))}
                </Select>
              </Field>
            </div>

            <Field
              label="API key"
              htmlFor="api_key"
              hint="Sent as Authorization: Bearer …. Never stored, never echoed, never in a share link."
            >
              <Input id="api_key" name="api_key" type="password" autoComplete="off" placeholder="sk-…" />
            </Field>

            <div className="grid grid-cols-3 gap-3">
              <Field label="Target RPS" htmlFor="target_rps">
                <Input id="target_rps" name="target_rps" type="number" min={1} defaultValue={100} required />
              </Field>
              <Field label="Duration (s)" htmlFor="duration_s">
                <Input id="duration_s" name="duration_s" type="number" min={1} defaultValue={60} required />
              </Field>
              <Field label="Connections" htmlFor="connections">
                <Input id="connections" name="connections" type="number" min={1} defaultValue={50} required />
              </Field>
            </div>

            <Field label="Load model" htmlFor="load_model">
              <Select id="load_model" name="load_model" defaultValue="open">
                <option value="open">open (fixed RPS schedule)</option>
                <option value="closed">closed (fixed concurrency)</option>
              </Select>
            </Field>

            {error && (
              <p className="text-sm text-danger">
                {error.message}
                {error.field && <span className="text-text-faint"> ({error.field})</span>}
              </p>
            )}

            <div className="flex items-center gap-3 pt-1">
              <Button type="submit" disabled={submitting}>
                {submitting ? 'Queuing…' : 'Queue run'}
              </Button>
              <Button type="button" variant="ghost" onClick={() => nav('/runs')}>
                Cancel
              </Button>
            </div>
          </form>
        </CardBody>
      </Card>
    </div>
  )
}
