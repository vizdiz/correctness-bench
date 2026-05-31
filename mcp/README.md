# mcp — MCP server for correctness-bench

Thin agent-callable surface over the control plane. Stdio transport. Each tool
is a wrapper over `contracts/api.md`; credentials forwarded to the target as
Bearer, never persisted by control (verified by the credential canary test).

## Install

```bash
npm install
npm run build      # tsc → dist/index.js
npm run smoke      # stdio client checks all 4 tools
```

## Tools

| tool | what it does | status |
|------|--------------|--------|
| `run_benchmark` | Build a CreateRunRequest from the agent's args, POST `/v1/runs`, return `{ run_id, status, dashboard_url }`. **Does not fire the engine** — that's manual today (`engine-worker --push-to <control> --run-id <id>`); coordinator will close the loop. | ✅ |
| `get_results` | GET `/v1/runs/:id` for the row, optionally subscribe `/v1/runs/:id/stream` for `live_window_s` seconds to gather fresh ticks, return a flat summary (`elapsed_s`, `achieved_rps_1s`, `pass_rate_pct`, `p50_ms`, `p99_ms`, ...). Safe to poll. | ✅ |
| `compare_apis` | Side-by-side compare of two runs (winner_by latency / correctness / cost). | ⏳ returns `NOT_IMPLEMENTED` until control plane ships `GET /v1/runs/:id/compare/:id2` |
| `regression_check` | Compare a freshly-completed run against a stored baseline. | ⏳ returns `NOT_IMPLEMENTED` until control ships `POST /v1/runs/:id/regression-check` |

## Env

| var | default | meaning |
|-----|---------|---------|
| `BENCH_CONTROL_URL` | `http://localhost:8000` | Control plane base URL the server talks to |
| `BENCH_WEB_URL` | `http://localhost:5173` | Used to build `dashboard_url` in responses |

## Use with an MCP client

Add to your MCP client config (Claude Desktop, etc.):

```json
{
  "mcpServers": {
    "correctness-bench": {
      "command": "node",
      "args": ["/abs/path/to/correctness-bench/mcp/dist/index.js"],
      "env": {
        "BENCH_CONTROL_URL": "http://localhost:8000"
      }
    }
  }
}
```

## Demo flow

```js
// 1. Create a run.
const create = await client.callTool({
  name: 'run_benchmark',
  arguments: {
    target: { url: 'https://api.example.com/v1/foo', api_key: 'sk-...' },
    target_rps: 200, duration_s: 30, connections: 20,
    expected_status: [200], max_latency_ms: 200,
    name: 'oai-vs-anthro',
  }
})
// → { run_id, dashboard_url, ... }

// 2. (Separately) engine fires it: docker run engine ... --push-to ... --run-id <id>
//    With the coordinator: this step goes away — `run_benchmark` will trigger it.

// 3. Poll get_results with a live window so SSE ticks are gathered.
const result = await client.callTool({
  name: 'get_results',
  arguments: { run_id, live_window_s: 3 }
})
// → { run, live: { pass_rate_pct, p99_ms, ... }, dashboard_url }
```

Verified end-to-end against the compose mock with `fast500 cliff=100 pct=50`:
the tool reports the exact 54.8% cliff and p99 ≈ 26.5 ms (matches the engine's own numbers).
