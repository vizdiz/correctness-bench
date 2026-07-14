# correctness-bench

Measure API **correctness as a continuous function of load**, not just latency. Point it at any HTTP endpoint, declare what "correct" means (status, JSON schema, value, max latency), and get the RPS where responses start going wrong - which is often well before latency degrades.

```
  Correctness vs offered RPS
   100% ●●●●●●●●●●●●●●●╮
                       ╰●───●───●───●───●───●───●───●     ← correctness collapses
     0%                    ────────────────────────────
                        ~150 rps (the cliff)      →  offered RPS

   Latency p99 ~26ms ───────────────────────────────────  ← latency stays flat
```

## Quick start

Requires Docker. Brings up six services plus a mock target to benchmark against:

```bash
docker compose up -d          # postgres, control, coordinator, 2x worker, mock, web
open http://localhost:5173    # dashboard
```

Create a run - it fires the worker fleet and streams results live:

```bash
curl -sX POST localhost:8000/v1/runs -H 'content-type: application/json' -d '{
  "name": "cliff-demo",
  "target": { "url": "http://mock:8080/api?mode=fast500&cliff_rps=150&pct=100&base_latency_ms=20",
              "method": "GET", "timeout_ms": 5000 },
  "target_rps": 300, "duration_s": 30, "load_model": "open", "connections": 30,
  "assert": { "expected_status": [200] }
}'
# -> {"run_id":"...","status":"running"}   then open /runs/<run_id> in the dashboard
```

Point `target.url` at your own endpoint to benchmark it. The dashboard also has **New run** and **Compare APIs** forms.

## Interfaces

**CLI** - `bench` from the [latest release](../../releases/latest) (linux-x86_64, macOS arm64/x86_64) or `cargo build --release -p bench-cli`:

```bash
export BENCH_CONTROL_URL=http://localhost:8000
export BENCH_API_KEY=sk-...          # Bearer to the target; never persisted

bench run -t 'http://mock:8080/api?mode=fast500&cliff_rps=150' -R 300 -d 30 --expected-status 200
bench regression <run_id> --baseline <baseline_id> --p99-delta-pct 10 --correctness-delta-pct 1
```

**CI gate** - fail a PR on correctness/p99 regression with the [correctness-gate action](.github/actions/correctness-gate):

```yaml
- uses: vizdiz/correctness-bench/.github/actions/correctness-gate@main
  with:
    control-url: ${{ secrets.BENCH_CONTROL_URL }}
    target-url: https://staging.example.com/api
    rps: '1000'
    duration: '60'
    baseline-run-id: ${{ vars.BENCH_BASELINE_RUN_ID }}
    api-key: ${{ secrets.TARGET_API_KEY }}
```

**Compare two APIs** - both run against one shared schedule window (same time, same conditions, each at full RPS):

```bash
curl -sX POST localhost:8000/v1/comparisons -H 'content-type: application/json' -d '{
  "name": "A vs B", "target_rps": 200, "duration_s": 30,
  "target_a": { "url": "https://api.vendor-a.com/v1/...", "method": "GET" },
  "target_b": { "url": "https://api.vendor-b.com/v1/...", "method": "GET" },
  "assert": { "expected_status": [200] }
}'
```

**MCP** - agent-callable tools: `run_benchmark`, `get_results`, `compare_apis`, `regression_check`, `list_templates`, `create_template`, `run_template`.

## What it measures

- **Correctness by load.** Pass rate and failures by class (transport, status, size, JSON-schema, value, latency), each bucketed by offered RPS at send time.
- **Coordinated-omission-correct latency.** Measured from a request's *intended* send time. Corrected and uncorrected HDR histograms are both kept; the delta is the omission error.
- **429 as a separate signal.** Rate limits are counted apart from correctness, with onset RPS surfaced and `Retry-After` honored.
- **Cost.** `cost_per_request_usd` gates runs at creation and auto-aborts them in flight if they exceed budget.
- **Fleet.** Load is split across workers and HDR-merged losslessly. A dead worker drops effective RPS with a `WORKER_LOST` warning (no fabricated load); abort tears the fleet down.

## Status

Phase 1, gate-verified. Each gate is objective and runnable; results are committed in `gates/results/`.

| gate | proves | result |
|------|--------|--------|
| #1 wrk2 agreement | corrected p50/p99 within ±5% of wrk2 | green - `gate1.json` (runner: `gates/gate1_containers.sh`) |
| #2 oracle | injected fail% == reported fail% | green - `gate2.json` (runner: `gates/gate2_oracle.sh`) |
| #3 fleet merge | two half-load workers ≈ one full-load run | green - `gate3.json` (runner: `gates/gate3_merge.sh`) |

Verified end to end:

- create a run → fleet fires → live SSE ticks → persisted history → completed
- abort propagates to the workers
- a killed worker degrades gracefully (`WORKER_LOST`, honest effective RPS)
- a restarted coordinator re-learns its fleet within ~10s
- concurrent A/B comparison under one schedule
- credential custody canary-verified (keys never reach the DB or logs)

Not in this build (out of Phase-1 scope): OpenTelemetry tracing, Jaeger/Grafana dashboards, browser visual regression. Logs are JSON to stdout (`docker compose logs <service>`); a Loki/Grafana stack is available behind `docker compose --profile observability up`, but the apps aren't OTLP-instrumented.

## Layout

| path | language | what |
|------|----------|------|
| [`engine/`](engine/)   | Rust | Worker + coordinator: COO scheduler, corrected/uncorrected HDR, gRPC fleet + lossless merge, abort, heartbeat death-detection. |
| [`control/`](control/) | Go | REST + SSE, lifecycle, credential custody, coordinator dispatch, cost guard, offload eval (schema/path/regex), Postgres + Timescale. |
| [`web/`](web/)         | TS/React | Vite + Tailwind: the four visualizations over SSE; new-run, compare, templates. |
| [`mock/`](mock/)       | Rust | Load-dependent failure injection: `healthy`, `fast500`, `truncate`, `wrong_value`, `slow_ok`, `ratelimit`. |
| [`cli/`](cli/)         | Rust | `bench` - control-plane client (`run`, `regression`). |
| [`mcp/`](mcp/)         | TS | MCP server: seven tools over the control REST. |
| [`contracts/`](contracts/) | - | Frozen: `bench.proto`, `api.md`, `schema.sql`. |
| [`gates/`](gates/)     | - | Acceptance gates, runners, result artifacts. |

## Hard rules

1. Workers never persist and never authenticate.
2. The control plane never generates load.
3. A slow DB write must never back-pressure the scheduler.
4. Target API keys: in memory for the run only, never persisted or logged, scrubbed from exports. Canary-verified.
5. Latency is measured from intended send time (gate #1's invariant).

## Reference

- [`latency-bench-architecture.md`](latency-bench-architecture.md) - design doc.
- [`contracts/`](contracts/) - frozen interfaces; open an issue rather than drift them.
- [`gates/`](gates/) - the acceptance gates.

Early-stage; no formal license yet.
