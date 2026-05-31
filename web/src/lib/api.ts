// Thin client of the control plane (contracts/api.md). Calls are same-origin
// `/v1/...`; the vite dev server proxies them to the control plane.

export type RunStatus =
  | 'draft'
  | 'validated'
  | 'queued'
  | 'running'
  | 'completed'
  | 'failed'
  | 'aborted'

export interface TargetInput {
  url: string
  method: string
  headers?: Record<string, string> // write-only; never returned
  body_base64?: string
  timeout_ms?: number
  verify_tls?: boolean
}

export interface CreateRunRequest {
  name?: string
  target: TargetInput
  target_rps: number
  duration_s: number
  warmup_s?: number
  load_model: 'open' | 'closed'
  connections: number
  keepalive?: boolean
  assert?: unknown
  rate_limit_policy?: unknown
  estimated_cost_usd?: number
  cost_per_request_usd?: number
}

export interface CreateRunResponse {
  run_id: string
  status: RunStatus
  estimated_cost_usd?: number
}

// Note: no `headers` field — the control plane never returns target.headers.
export interface RunView {
  run_id: string
  name?: string
  status: RunStatus
  target: { url: string; method: string }
  target_rps: number
  duration_s?: number
  effective_rps?: number
  created_at?: string
  started_at?: string
  completed_at?: string
  elapsed_s?: number
}

export interface RunSummary {
  run_id: string
  name?: string
  status: RunStatus
  target_rps: number
  duration_s: number
  created_at: string
}

export interface ListRunsResponse {
  runs: RunSummary[]
  next_cursor: string | null
}

// Live SSE tick event from /v1/runs/:id/stream. Shape mirrors engine's Tick.
// Will grow toward api.md SSE tick shape (percentiles, buckets) as engine adds.
export interface Tick {
  elapsed_s: number
  achieved_rps_1s: number
  completed_total: number
  pass_total: number
  fail_status_total: number
  /** Counts FOR THIS TICK only — per-second deltas. The headline green line
      plots `this_tick.pass / this_tick.total` to show the per-second cliff. */
  this_tick: {
    total: number
    pass: number
    fail_status: number
    fail_latency: number
    fail_size: number
    fail_content_type: number
  }
  /** Running corrected p50/p99 latency snapshot (api.md `percentiles_so_far`). */
  percentiles_so_far: { p50_us: number; p99_us: number }
  /** Per-RPS bucket entries attributable to this tick. Clients accumulate
      across ticks to build the correctness-vs-load curve. */
  buckets?: Bucket[]
  ts?: string
}

export interface Bucket {
  rps_lo: number
  rps_hi: number
  total: number
  pass: number
  fail_status: number
  fail_latency: number
  fail_size: number
  fail_content_type: number
}

export interface ApiErrorBody {
  error: { code: string; message: string; field?: string; request_id: string }
}

export class ApiError extends Error {
  code: string
  field?: string
  constructor(body: ApiErrorBody) {
    super(body.error.message)
    this.code = body.error.code
    this.field = body.error.field
  }
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    ...init,
    headers: { 'Content-Type': 'application/json', ...(init?.headers ?? {}) },
  })
  const text = await res.text()
  const json = text ? JSON.parse(text) : null
  if (!res.ok) {
    if (json && json.error) throw new ApiError(json as ApiErrorBody)
    throw new Error(`HTTP ${res.status}`)
  }
  return json as T
}

export const api = {
  createRun: (body: CreateRunRequest) =>
    req<CreateRunResponse>('/v1/runs', { method: 'POST', body: JSON.stringify(body) }),

  getRun: (id: string) => req<RunView>(`/v1/runs/${id}`),

  listRuns: (params: { status?: string; limit?: number; cursor?: string } = {}) => {
    const q = new URLSearchParams()
    if (params.status) q.set('status', params.status)
    if (params.limit) q.set('limit', String(params.limit))
    if (params.cursor) q.set('cursor', params.cursor)
    const qs = q.toString()
    return req<ListRunsResponse>(`/v1/runs${qs ? `?${qs}` : ''}`)
  },

  abortRun: (id: string) =>
    req<{ status: string; aborted_at: string }>(`/v1/runs/${id}/abort`, { method: 'POST' }),
}
