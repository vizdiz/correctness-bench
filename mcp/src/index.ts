#!/usr/bin/env node
// MCP server exposing the correctness-bench control plane as agent-callable
// tools. Stdio transport. Each tool is a thin wrapper over contracts/api.md
// — credentials forwarded to the target as Bearer, never stored.

import { Server } from '@modelcontextprotocol/sdk/server/index.js'
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  type CallToolRequest,
  type Tool,
} from '@modelcontextprotocol/sdk/types.js'

const CONTROL_URL = (process.env.BENCH_CONTROL_URL ?? 'http://localhost:8000').replace(/\/$/, '')
const WEB_URL = (process.env.BENCH_WEB_URL ?? 'http://localhost:5173').replace(/\/$/, '')

const tools: Tool[] = [
  {
    name: 'run_benchmark',
    description:
      'Create a benchmark run via the control plane. Returns { run_id, dashboard_url }. The run is queued; an engine worker fires it (today: manually via `engine-worker --push-to <control> --run-id <id>`; with the coordinator: automatic). Use `get_results` to poll status + accumulated metrics.',
    inputSchema: {
      type: 'object',
      required: ['target', 'target_rps', 'duration_s'],
      properties: {
        target: {
          type: 'object',
          required: ['url'],
          properties: {
            url: { type: 'string', description: 'Target URL to benchmark.' },
            method: { type: 'string', default: 'GET', description: 'HTTP method.' },
            api_key: {
              type: 'string',
              description:
                'Sent as `Authorization: Bearer <api_key>` to the target. Never persisted, scrubbed from logs and exports (verified by the credential canary test).',
            },
          },
        },
        target_rps: { type: 'number', description: 'Target requests per second.' },
        duration_s: { type: 'integer', description: 'Run duration in seconds.' },
        connections: { type: 'integer', default: 50, description: 'Open connections.' },
        load_model: { type: 'string', enum: ['open', 'closed'], default: 'open' },
        expected_status: {
          type: 'array',
          items: { type: 'integer' },
          description:
            'Inline status-tier assertion. Empty = any 2xx is pass. Non-empty = status must be in the list.',
        },
        max_latency_ms: {
          type: 'integer',
          description: 'Inline latency-tier assertion. Any corrected latency > this is fail_latency.',
        },
        name: { type: 'string', description: 'Optional run name shown in the UI.' },
      },
    },
  },
  {
    name: 'get_results',
    description:
      'Fetch the current state of a run by id. Returns the RunView from GET /v1/runs/:id plus a snapshot of accumulated SSE ticks (rps, pass rate, percentiles). Safe to poll.',
    inputSchema: {
      type: 'object',
      required: ['run_id'],
      properties: {
        run_id: { type: 'string', description: 'UUID returned by run_benchmark.' },
        live_window_s: {
          type: 'integer',
          default: 3,
          description:
            'Optional: subscribe to SSE for up to this many seconds to gather fresh ticks before returning. 0 = REST-only.',
        },
      },
    },
  },
  {
    name: 'compare_apis',
    description:
      'Side-by-side compare of two runs. Wraps GET /v1/runs/:id/compare/:id2. Returns both run views plus winner_by (latency_p99, correctness, cost_per_request, tail_stability, rate_limit_onset) and fairness flags. Best paired with `regression_check` when one side is a baseline.',
    inputSchema: {
      type: 'object',
      required: ['run_id_a', 'run_id_b'],
      properties: {
        run_id_a: { type: 'string' },
        run_id_b: { type: 'string' },
      },
    },
  },
  {
    name: 'regression_check',
    description:
      'Compare a finalized candidate run against a finalized baseline. Wraps POST /v1/runs/:id/regression-check. Returns { passed, p99_delta_pct, correctness_delta_pct, cliff_rps_delta, details }. Tolerance defaults match api.md: p99 +10%, correctness -1pp. Both runs must already be finalized (status=completed).',
    inputSchema: {
      type: 'object',
      required: ['run_id', 'baseline_run_id'],
      properties: {
        run_id: { type: 'string' },
        baseline_run_id: { type: 'string' },
        thresholds: {
          type: 'object',
          properties: {
            p99_delta_pct: { type: 'number', default: 10 },
            correctness_delta_pct: { type: 'number', default: 1 },
          },
        },
      },
    },
  },
  {
    name: 'list_templates',
    description:
      'List saved run templates. Wraps GET /v1/templates. Templates store redacted run specs (secrets replaced with "***"); re-supply them via `run_template`.',
    inputSchema: { type: 'object', properties: {} },
  },
  {
    name: 'create_template',
    description:
      'Save a run spec as a re-runnable template. Wraps POST /v1/templates. target.api_key is forwarded as the Authorization header on the stored spec; control scrubs it to "***" before persistence.',
    inputSchema: {
      type: 'object',
      required: ['name', 'target', 'target_rps', 'duration_s'],
      properties: {
        name: { type: 'string' },
        target: {
          type: 'object',
          required: ['url'],
          properties: {
            url: { type: 'string' },
            method: { type: 'string', default: 'GET' },
            api_key: {
              type: 'string',
              description: 'Scrubbed by control before persistence; never reaches storage.',
            },
          },
        },
        target_rps: { type: 'number' },
        duration_s: { type: 'integer' },
        connections: { type: 'integer', default: 50 },
        load_model: { type: 'string', enum: ['open', 'closed'], default: 'open' },
        expected_status: { type: 'array', items: { type: 'integer' } },
        max_latency_ms: { type: 'integer' },
      },
    },
  },
  {
    name: 'run_template',
    description:
      'Fork a stored template into a fresh run. Wraps POST /v1/templates/:id/run. `headers.api_key` (when set) is forwarded as the target Authorization header so the redacted "***" placeholder is replaced; other overrides (target_rps, duration_s) fall back to the template defaults when omitted.',
    inputSchema: {
      type: 'object',
      required: ['template_id'],
      properties: {
        template_id: { type: 'string' },
        api_key: {
          type: 'string',
          description: 'Re-supply for the Authorization header that was stored as "***".',
        },
        headers: {
          type: 'object',
          additionalProperties: { type: 'string' },
          description: 'Other headers to merge into the template (e.g., X-Override).',
        },
        name: { type: 'string' },
        target_rps: { type: 'number' },
        duration_s: { type: 'integer' },
      },
    },
  },
]

interface CreateRunResponse {
  run_id: string
  status: string
}

interface RunView {
  run_id: string
  name?: string
  status: string
  target: { url: string; method: string }
  target_rps: number
  duration_s?: number
  created_at?: string
  effective_rps?: number
  cost_per_request_usd?: number
  offload?: {
    pass: number
    fail_schema: number
    fail_value: number
    fail_regex: number
  }
  final?: {
    corrected: { p50_us: number; p95_us: number; p99_us: number; p999_us?: number }
    total_requests: number
    total_pass: number
    correctness_pct: number
    cliff_rps?: number
  }
}

interface CompareResponse {
  a: RunView
  b: RunView
  winner_by: {
    latency_p99: string
    correctness: string
    cost_per_request: string
    tail_stability: string
    rate_limit_onset: string
  }
  fairness: {
    interleaved: boolean
    same_assert_spec: boolean
    same_load_shape: boolean
  }
}

interface RegressionResponse {
  passed: boolean
  p99_delta_pct: number
  correctness_delta_pct: number
  cliff_rps_delta: number
  details: string
}

interface TemplateView {
  id: string
  name: string
  spec: Record<string, unknown>
  created_at: string
  last_used_at?: string
}

interface Tick {
  elapsed_s: number
  achieved_rps_1s: number
  completed_total: number
  pass_total: number
  fail_status_total: number
  this_tick: {
    total: number
    pass: number
    fail_status: number
    fail_latency: number
    fail_size: number
    fail_content_type: number
  }
  percentiles_so_far: { p50_us: number; p99_us: number }
}

async function controlFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const url = `${CONTROL_URL}${path}`
  let resp: Response
  try {
    resp = await fetch(url, init)
  } catch (e) {
    throw new Error(`control plane at ${CONTROL_URL} unreachable: ${(e as Error).message}`)
  }
  if (!resp.ok) {
    const body = await resp.text().catch(() => '')
    throw new Error(`${init?.method ?? 'GET'} ${path} → HTTP ${resp.status}: ${body}`)
  }
  return (await resp.json()) as T
}

function txt(text: string) {
  return { content: [{ type: 'text', text }] as const }
}

async function runBenchmark(args: any) {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' }
  const targetHeaders: Record<string, string> = {}
  if (args.target?.api_key) {
    targetHeaders['Authorization'] = `Bearer ${args.target.api_key}`
  }
  const assert: Record<string, unknown> = {}
  if (Array.isArray(args.expected_status) && args.expected_status.length > 0) {
    assert.expected_status = args.expected_status
  }
  if (typeof args.max_latency_ms === 'number') {
    assert.max_latency_us = args.max_latency_ms * 1000
  }

  const body = {
    name: args.name,
    target: {
      url: args.target.url,
      method: (args.target.method ?? 'GET').toUpperCase(),
      headers: targetHeaders,
      timeout_ms: 30_000,
      verify_tls: true,
    },
    target_rps: args.target_rps,
    duration_s: args.duration_s,
    load_model: args.load_model ?? 'open',
    connections: args.connections ?? 50,
    keepalive: true,
    assert,
    rate_limit_policy: { action: 'backoff', record_onset: true },
  }

  const res = await controlFetch<CreateRunResponse>('/v1/runs', {
    method: 'POST',
    headers,
    body: JSON.stringify(body),
  })
  return txt(
    JSON.stringify(
      {
        run_id: res.run_id,
        status: res.status,
        dashboard_url: `${WEB_URL}/runs/${res.run_id}`,
        note:
          'Run queued. An engine worker must be firing this run (today: manually via `engine-worker --push-to ' +
          CONTROL_URL +
          ' --run-id ' +
          res.run_id +
          '`; with coordinator: automatic). Call `get_results` to fetch live metrics.',
      },
      null,
      2,
    ),
  )
}

async function getResults(args: any) {
  const id: string = args.run_id
  if (!id || typeof id !== 'string') throw new Error('run_id required')
  const view = await controlFetch<RunView>(`/v1/runs/${id}`)
  const windowS: number = typeof args.live_window_s === 'number' ? args.live_window_s : 3

  let ticks: Tick[] = []
  if (windowS > 0) {
    ticks = await collectTicks(id, windowS * 1000)
  }
  const last = ticks[ticks.length - 1]
  const passRate =
    last && last.completed_total > 0 ? (last.pass_total / last.completed_total) * 100 : null
  const summary = {
    run: {
      run_id: view.run_id,
      name: view.name,
      status: view.status,
      target: view.target,
      target_rps: view.target_rps,
      created_at: view.created_at,
      cost_per_request_usd: view.cost_per_request_usd,
      offload: view.offload,
      final: view.final,
    },
    live: last
      ? {
          ticks_received: ticks.length,
          elapsed_s: last.elapsed_s,
          achieved_rps_1s: last.achieved_rps_1s,
          completed_total: last.completed_total,
          pass_total: last.pass_total,
          fail_status_total: last.fail_status_total,
          pass_rate_pct: passRate,
          p50_ms: last.percentiles_so_far.p50_us / 1000,
          p99_ms: last.percentiles_so_far.p99_us / 1000,
        }
      : { ticks_received: 0, note: 'No ticks received in the live window; engine may not be firing this run.' },
    dashboard_url: `${WEB_URL}/runs/${view.run_id}`,
  }
  return txt(JSON.stringify(summary, null, 2))
}

/** Subscribe to /v1/runs/:id/stream for up to windowMs, collect any tick events. */
async function collectTicks(runId: string, windowMs: number): Promise<Tick[]> {
  const url = `${CONTROL_URL}/v1/runs/${runId}/stream`
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), windowMs)
  const ticks: Tick[] = []
  try {
    const resp = await fetch(url, {
      headers: { Accept: 'text/event-stream' },
      signal: controller.signal,
    })
    if (!resp.ok || !resp.body) return ticks
    const reader = resp.body.getReader()
    const decoder = new TextDecoder()
    let buf = ''
    while (true) {
      const { value, done } = await reader.read()
      if (done) break
      buf += decoder.decode(value, { stream: true })
      let idx: number
      while ((idx = buf.indexOf('\n\n')) >= 0) {
        const block = buf.slice(0, idx)
        buf = buf.slice(idx + 2)
        const ev = parseSse(block)
        if (ev && ev.name === 'tick' && ev.data) {
          try {
            ticks.push(JSON.parse(ev.data) as Tick)
          } catch {
            /* skip malformed */
          }
        }
      }
    }
  } catch {
    /* abort or network end — that's the timer firing */
  } finally {
    clearTimeout(timer)
  }
  return ticks
}

function parseSse(block: string): { name: string; data: string } | null {
  let name = 'message'
  let data = ''
  for (const raw of block.split('\n')) {
    const line = raw.replace(/\r$/, '')
    if (line.startsWith('data:')) {
      if (data.length > 0) data += '\n'
      data += line.slice(5).replace(/^ /, '')
    } else if (line.startsWith('event:')) {
      name = line.slice(6).replace(/^ /, '')
    }
  }
  return data ? { name, data } : null
}

async function compareApis(args: any) {
  const a: string = args.run_id_a
  const b: string = args.run_id_b
  if (!a || !b) throw new Error('run_id_a and run_id_b required')
  const data = await controlFetch<CompareResponse>(`/v1/runs/${a}/compare/${b}`)
  // Compact form: caller wants the headline, not the full RunView twice. Keep
  // winner_by + fairness + per-side correctness/p99/cost so the agent can act.
  const compact = {
    a: compactSide(data.a),
    b: compactSide(data.b),
    winner_by: data.winner_by,
    fairness: data.fairness,
    dashboard_url: `${WEB_URL}/compare/${a}/${b}`,
  }
  return txt(JSON.stringify(compact, null, 2))
}

function compactSide(v: RunView) {
  return {
    run_id: v.run_id,
    name: v.name,
    status: v.status,
    target: v.target,
    target_rps: v.target_rps,
    cost_per_request_usd: v.cost_per_request_usd,
    correctness_pct: v.final?.correctness_pct,
    p50_ms: v.final ? v.final.corrected.p50_us / 1000 : undefined,
    p95_ms: v.final ? v.final.corrected.p95_us / 1000 : undefined,
    p99_ms: v.final ? v.final.corrected.p99_us / 1000 : undefined,
    cliff_rps: v.final?.cliff_rps,
    offload: v.offload,
  }
}

async function regressionCheck(args: any) {
  const id: string = args.run_id
  const baseline: string = args.baseline_run_id
  if (!id || !baseline) throw new Error('run_id and baseline_run_id required')
  const thresholds = {
    p99_delta_pct: args.thresholds?.p99_delta_pct ?? 10,
    correctness_delta_pct: args.thresholds?.correctness_delta_pct ?? 1,
  }
  const data = await controlFetch<RegressionResponse>(
    `/v1/runs/${id}/regression-check`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ baseline_run_id: baseline, thresholds }),
    },
  )
  return txt(
    JSON.stringify(
      {
        ...data,
        candidate: id,
        baseline,
        thresholds,
        dashboard_url: `${WEB_URL}/compare/${id}/${baseline}`,
      },
      null,
      2,
    ),
  )
}

async function listTemplates(_args: any) {
  const data = await controlFetch<{ templates: TemplateView[] }>('/v1/templates')
  return txt(
    JSON.stringify(
      {
        templates: data.templates.map((t) => ({
          id: t.id,
          name: t.name,
          target: (t.spec as any)?.target,
          target_rps: (t.spec as any)?.target_rps,
          duration_s: (t.spec as any)?.duration_s,
          created_at: t.created_at,
          last_used_at: t.last_used_at,
        })),
      },
      null,
      2,
    ),
  )
}

async function createTemplate(args: any) {
  if (!args.name) throw new Error('name required')
  const targetHeaders: Record<string, string> = {}
  if (args.target?.api_key) {
    targetHeaders['Authorization'] = `Bearer ${args.target.api_key}`
  }
  const assert: Record<string, unknown> = {}
  if (Array.isArray(args.expected_status) && args.expected_status.length > 0) {
    assert.expected_status = args.expected_status
  }
  if (typeof args.max_latency_ms === 'number') {
    assert.max_latency_us = args.max_latency_ms * 1000
  }
  const spec = {
    name: args.name,
    target: {
      url: args.target.url,
      method: (args.target.method ?? 'GET').toUpperCase(),
      headers: targetHeaders,
      timeout_ms: 30_000,
      verify_tls: true,
    },
    target_rps: args.target_rps,
    duration_s: args.duration_s,
    load_model: args.load_model ?? 'open',
    connections: args.connections ?? 50,
    keepalive: true,
    assert,
    rate_limit_policy: { action: 'backoff', record_onset: true },
  }
  const data = await controlFetch<TemplateView>('/v1/templates', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name: args.name, spec }),
  })
  return txt(
    JSON.stringify(
      {
        id: data.id,
        name: data.name,
        created_at: data.created_at,
        dashboard_url: `${WEB_URL}/templates`,
      },
      null,
      2,
    ),
  )
}

async function runTemplate(args: any) {
  if (!args.template_id) throw new Error('template_id required')
  const headers: Record<string, string> = { ...(args.headers ?? {}) }
  if (args.api_key) headers['Authorization'] = `Bearer ${args.api_key}`
  const body: Record<string, unknown> = {}
  if (Object.keys(headers).length > 0) body.headers = headers
  if (args.name) body.name = args.name
  if (typeof args.target_rps === 'number') body.target_rps = args.target_rps
  if (typeof args.duration_s === 'number') body.duration_s = args.duration_s
  const data = await controlFetch<CreateRunResponse>(
    `/v1/templates/${args.template_id}/run`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    },
  )
  return txt(
    JSON.stringify(
      {
        run_id: data.run_id,
        status: data.status,
        dashboard_url: `${WEB_URL}/runs/${data.run_id}`,
      },
      null,
      2,
    ),
  )
}

const server = new Server(
  { name: 'correctness-bench', version: '0.1.0' },
  { capabilities: { tools: {} } },
)

server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools }))

server.setRequestHandler(CallToolRequestSchema, async (req: CallToolRequest) => {
  const { name, arguments: args = {} } = req.params
  try {
    switch (name) {
      case 'run_benchmark':
        return await runBenchmark(args)
      case 'get_results':
        return await getResults(args)
      case 'compare_apis':
        return await compareApis(args)
      case 'regression_check':
        return await regressionCheck(args)
      case 'list_templates':
        return await listTemplates(args)
      case 'create_template':
        return await createTemplate(args)
      case 'run_template':
        return await runTemplate(args)
      default:
        throw new Error(`unknown tool: ${name}`)
    }
  } catch (e) {
    return txt(
      JSON.stringify(
        {
          error: {
            code: 'TOOL_ERROR',
            message: (e as Error).message,
          },
        },
        null,
        2,
      ),
    )
  }
})

const transport = new StdioServerTransport()
await server.connect(transport)
