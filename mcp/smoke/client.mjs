#!/usr/bin/env node
// Smoke test: connect to ./dist/index.js over stdio, list tools, call
// run_benchmark + get_results, print results. Exits non-zero on failure.
// Requires a control plane reachable at $BENCH_CONTROL_URL (default
// http://localhost:8000) and an engine worker firing for the live test to
// see ticks (otherwise get_results just returns "no ticks").

import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js'
import { fileURLToPath } from 'node:url'
import { resolve, dirname } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const serverScript = resolve(here, '..', 'dist', 'index.js')

const transport = new StdioClientTransport({
  command: 'node',
  args: [serverScript],
  env: { ...process.env },
})

const client = new Client(
  { name: 'mcp-smoke', version: '0.1.0' },
  { capabilities: {} },
)

let failures = 0
function check(ok, label) {
  console.log(`${ok ? '  ok' : 'FAIL'}  ${label}`)
  if (!ok) failures += 1
}

await client.connect(transport)

const { tools } = await client.listTools()
const names = tools.map((t) => t.name).sort()
check(
  JSON.stringify(names) === JSON.stringify(['compare_apis', 'get_results', 'regression_check', 'run_benchmark']),
  `tools/list returned the 4 expected tools (got ${names.join(',')})`,
)

let runId = null
try {
  const result = await client.callTool({
    name: 'run_benchmark',
    arguments: {
      target: { url: 'http://mock:8080/api?mode=healthy&base_latency_ms=20' },
      target_rps: 100,
      duration_s: 4,
      name: 'mcp-smoke',
    },
  })
  const text = result.content?.[0]?.type === 'text' ? result.content[0].text : ''
  const payload = JSON.parse(text)
  runId = payload.run_id
  check(typeof runId === 'string' && runId.length > 10, `run_benchmark returned run_id (${runId})`)
  check(
    typeof payload.dashboard_url === 'string' && payload.dashboard_url.includes(runId),
    `dashboard_url contains the run_id`,
  )
} catch (e) {
  check(false, `run_benchmark threw: ${e.message}`)
}

if (runId) {
  const result = await client.callTool({
    name: 'get_results',
    arguments: { run_id: runId, live_window_s: 1 },
  })
  const text = result.content?.[0]?.type === 'text' ? result.content[0].text : ''
  const payload = JSON.parse(text)
  check(payload.run?.run_id === runId, `get_results returned the right run`)
  check(payload.run?.status === 'queued', `run is queued (status=${payload.run?.status})`)
  check(
    typeof payload.dashboard_url === 'string' && payload.dashboard_url.includes(runId),
    `get_results includes dashboard_url`,
  )
}

// Stubs return NOT_IMPLEMENTED until control gets the endpoints.
for (const name of ['compare_apis', 'regression_check']) {
  const result = await client.callTool({
    name,
    arguments: name === 'compare_apis' ? { run_id_a: 'x', run_id_b: 'y' } : { run_id: 'x', baseline_run_id: 'y' },
  })
  const text = result.content?.[0]?.type === 'text' ? result.content[0].text : ''
  let payload = {}
  try {
    payload = JSON.parse(text)
  } catch {}
  check(payload?.error?.code === 'NOT_IMPLEMENTED', `${name} returns NOT_IMPLEMENTED stub`)
}

await client.close()

if (failures > 0) {
  console.error(`\n${failures} check(s) failed`)
  process.exit(1)
}
console.log('\nall checks passed')
