import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api, type TemplateView } from '../lib/api'
import { Button } from '../components/ui/Button'
import { Card, CardBody } from '../components/ui/Card'
import { Field, Input } from '../components/ui/Input'
import { shortTime } from '../lib/format'

/**
 * /templates — saved re-runnable run specs. Lists each row with target +
 * load shape. The "Run" button opens an inline panel for re-supplying secrets
 * (the "***" placeholder values), then POSTs to /v1/templates/:id/run and
 * navigates to the new run.
 */
export function Templates() {
  const [list, setList] = useState<TemplateView[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [runFor, setRunFor] = useState<TemplateView | null>(null)
  const nav = useNavigate()

  function load() {
    setError(null)
    api
      .listTemplates()
      .then((r) => setList(r.templates ?? []))
      .catch((e) => setError(e.message))
  }
  useEffect(load, [])

  if (error) {
    return (
      <Card className="border-danger/40 bg-danger/5 px-5 py-4 text-sm text-danger">
        {error}
      </Card>
    )
  }
  if (!list) return <p className="text-sm text-text-faint">Loading templates…</p>

  return (
    <div>
      <header className="mb-6 flex items-start justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Templates</h1>
          <p className="mt-1 text-xs text-text-faint">
            Saved run specs. Secrets in target headers are stored as <code>"***"</code> —
            re-supply on Run.
          </p>
        </div>
      </header>

      {list.length === 0 ? (
        <Card>
          <CardBody className="text-sm text-text-faint">
            No templates yet. Create one from{' '}
            <a className="underline" href="/runs/new">/runs/new</a> (coming: a "Save as
            template" toggle on that page) or via{' '}
            <code>POST /v1/templates</code>.
          </CardBody>
        </Card>
      ) : (
        <div className="grid grid-cols-1 gap-3">
          {list.map((t) => (
            <Card key={t.id}>
              <CardBody className="flex flex-wrap items-center justify-between gap-3">
                <div className="min-w-0 flex-1">
                  <div className="font-medium text-text">{t.name}</div>
                  <div className="mt-0.5 truncate font-mono text-xs text-text-faint">
                    {t.spec.target.method} {t.spec.target.url}
                  </div>
                  <div className="mt-1 text-xs text-text-muted">
                    {t.spec.target_rps} RPS · {t.spec.duration_s}s · created {shortTime(t.created_at)}
                    {t.last_used_at && <> · last used {shortTime(t.last_used_at)}</>}
                  </div>
                </div>
                <div className="flex items-center gap-2">
                  <Button onClick={() => setRunFor(t)}>Run</Button>
                </div>
              </CardBody>
              {runFor?.id === t.id && (
                <RunInlinePanel
                  template={t}
                  onCancel={() => setRunFor(null)}
                  onLaunched={(runId) => nav(`/runs/${runId}`)}
                />
              )}
            </Card>
          ))}
        </div>
      )}
    </div>
  )
}

function RunInlinePanel({
  template,
  onCancel,
  onLaunched,
}: {
  template: TemplateView
  onCancel: () => void
  onLaunched: (runId: string) => void
}) {
  // Auto-detect which headers need re-supplying ("***" placeholders).
  const redactedKeys = Object.entries(template.spec.target.headers ?? {})
    .filter(([, v]) => v === '***')
    .map(([k]) => k)
  const [headers, setHeaders] = useState<Record<string, string>>(() =>
    Object.fromEntries(redactedKeys.map((k) => [k, ''])),
  )
  const [name, setName] = useState<string>('')
  const [targetRPS, setTargetRPS] = useState<number | ''>('')
  const [durationS, setDurationS] = useState<number | ''>('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function launch() {
    setBusy(true)
    setError(null)
    try {
      const body: any = {}
      if (Object.keys(headers).length > 0) body.headers = headers
      if (name) body.name = name
      if (typeof targetRPS === 'number') body.target_rps = targetRPS
      if (typeof durationS === 'number') body.duration_s = durationS
      const r = await api.runTemplate(template.id, body)
      onLaunched(r.run_id)
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="border-t border-border bg-surface-2/30 px-5 py-4">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <Field label="Run name (optional)">
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={template.name}
          />
        </Field>
        <Field label="Target RPS (default from template)">
          <Input
            type="number"
            value={targetRPS === '' ? '' : String(targetRPS)}
            onChange={(e) =>
              setTargetRPS(e.target.value === '' ? '' : Number(e.target.value))
            }
            placeholder={String(template.spec.target_rps)}
          />
        </Field>
        <Field label="Duration s">
          <Input
            type="number"
            value={durationS === '' ? '' : String(durationS)}
            onChange={(e) =>
              setDurationS(e.target.value === '' ? '' : Number(e.target.value))
            }
            placeholder={String(template.spec.duration_s)}
          />
        </Field>
      </div>
      {redactedKeys.length > 0 && (
        <div className="mt-4">
          <div className="text-xs uppercase tracking-wide text-text-faint">
            Re-supply redacted headers
          </div>
          <div className="mt-1 grid grid-cols-1 gap-2 sm:grid-cols-2">
            {redactedKeys.map((k) => (
              <Field key={k} label={k}>
                <Input
                  type="password"
                  value={headers[k] ?? ''}
                  onChange={(e) =>
                    setHeaders((h) => ({ ...h, [k]: e.target.value }))
                  }
                  placeholder="paste secret"
                />
              </Field>
            ))}
          </div>
        </div>
      )}
      {error && <p className="mt-3 text-sm text-danger">{error}</p>}
      <div className="mt-4 flex justify-end gap-2">
        <Button variant="secondary" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
        <Button onClick={launch} disabled={busy}>
          {busy ? 'Launching…' : 'Launch run'}
        </Button>
      </div>
    </div>
  )
}
