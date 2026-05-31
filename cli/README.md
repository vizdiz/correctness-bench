# bench — CLI

Thin client of the control plane. Builds a run spec from flags, POSTs it to
`/v1/runs`, subscribes to `/v1/runs/:id/stream` (SSE), renders a live
per-second dashboard, and prints a final summary on completion. **Not a load
generator of its own** — that's `engine-worker`. Currently the engine has to
be fired separately (with matching `--push-to` + `--run-id`); the coordinator
will close that loop later.

## Usage

```bash
bench --target https://api.example.com/v1/foo --rps 100 --duration-s 30 \
      --expected-status 200 --max-latency-ms 200 \
      --key sk-...                     # API key, Bearer-injected; never persisted
```

Output (live):
```
bench: created run 89a8b0af-...
       dashboard: http://localhost:5173/runs/89a8b0af-...
       streaming live ticks (30s)...
  t=  1s   rps=  196   pass= 74.5% (  146/  196)   p50= 24.5ms   p99= 27.2ms
  t=  2s   rps=  200   pass= 50.0% (  100/  200)   p50= 24.6ms   p99= 30.5ms
  ...
  run            89a8b0af-...
  name           ...
  target         GET ...
  ticks received 30
  requests       5996
  pass           4500 / 5996 = 75.1% (cumulative)
  fail by tier   status=1496 latency=0 size=0 content_type=0
  latency        p50=24.6ms  p99=27.2ms
  dashboard      http://localhost:5173/runs/89a8b0af-...
```

## Flags

| flag | default | meaning |
|------|---------|---------|
| `-t, --target` | (required) | Target URL |
| `-m, --method` | `GET` | HTTP method |
| `-R, --rps` | `100` | Target RPS |
| `-d, --duration-s` | `30` | Run duration (s) |
| `-c, --connections` | `50` | Open connections |
| `--load-model` | `open` | `open` or `closed` |
| `--expected-status N` | (any 2xx) | Status assertion; repeat for multiple |
| `--max-latency-ms` | (off) | Inline latency tier |
| `--name` | (none) | Run name |
| `--key` (or `$BENCH_API_KEY`) | (none) | `Authorization: Bearer <key>` to target |
| `--control` (or `$BENCH_CONTROL_URL`) | `http://localhost:8000` | Control plane base URL |
| `--web` (or `$BENCH_WEB_URL`) | `http://localhost:5173` | Dashboard base URL |
| `--timeout-ms` | `30000` | Per-request timeout (passed to control) |

## Exit codes

- **0** — run completed (at least one tick received within `duration_s + 5s`)
- **1** — engine error: aborted by Ctrl-C, or zero ticks received in time
- **2** — bad args (clap) / control plane unreachable / spec rejected by control

## Out of scope (deferred)

- No threshold gating / `--fail-if` (see `.claude/agents/cli.md`: threshold logic
  lives in a separate future query-language project).
- No auto-firing the engine — separate `engine-worker --push-to ... --run-id ...`
  is required today. Coordinator will close the loop.
