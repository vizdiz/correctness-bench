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
      "Side-by-side compare of two runs (winner_by latency / correctness / cost). Wraps GET /v1/runs/:id/compare/:id2. NOT IMPLEMENTED YET in the control plane — this tool returns a clear NOT_IMPLEMENTED error until that endpoint lands.",
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
      'Compare a freshly-completed run against a stored baseline. Wraps POST /v1/runs/:id/regression-check. NOT IMPLEMENTED YET in the control plane — this tool returns NOT_IMPLEMENTED until that endpoint lands.',
    inputSchema: {
      type: 'object',
      required: ['run_id', 'baseline_run_id'],
      properties: {
        run_id: { type: 'string' },
        baseline_run_id: { type: 'string' },
        thresholds: {
          type: 'object',
          properties: {
            p99_delta_pct: { type: 'number' },
            correctness_delta_pct: { type: 'number' },
          },
        },
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

function notImplemented(toolName: string) {
  return txt(
    JSON.stringify(
      {
        error: {
          code: 'NOT_IMPLEMENTED',
          message: `Tool '${toolName}' depends on a control-plane endpoint that hasn't shipped yet. Tracked.`,
        },
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
        return notImplemented('compare_apis')
      case 'regression_check':
        return notImplemented('regression_check')
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
