# correctness-bench

**Find the load at which an API's responses start going wrong - while its latency still looks healthy.**

Every load test measures latency vs load and treats correctness as a checkbox. The failure that actually hurts is the one they miss: a service that stays fast while it quietly starts returning wrong answers past some request rate. wrk2 counts a fast HTTP 500 as a win. Postman sends one request. This tool measures **correctness as a continuous function of load** and shows you the cliff.

```
  Correctness vs offered RPS
   100% ●●●●●●●●●●●●●●●╮
                       ╰●───●───●───●───●───●───●───●     ← correctness collapses
     0%                    ────────────────────────────
                        ~150 rps (the cliff)      →  offered RPS

   Latency p99 ~26ms ───────────────────────────────────  ← latency stays flat, healthy
```

Point it at any HTTP endpoint - a SaaS API, an LLM provider, your service in staging - declare what "correct" means (status, JSON schema, a value, max latency), and watch correctness as load climbs. You learn the RPS where it breaks, not just the latency distribution.

---

## Quick start

Requires Docker. This brings up the six product services plus a built-in mock target to benchmark against:

```bash
docker compose up -d          # postgres, control, coordinator, 2x worker, mock, web
open http://localhost:5173    # the dashboard
```

Create a run - it fires the worker fleet automatically and streams the cliff live:

```bash
curl -sX POST localhost:8000/v1/runs -H 'content-type: application/json' -d '{
  "name": "cliff-demo",
  "target": { "url": "http://mock:8080/api?mode=fast500&cliff_rps=150&pct=100&base_latency_ms=20",
              "method": "GET", "timeout_ms": 5000 },
  "target_rps": 300, "duration_s": 30, "load_model": "open", "connections": 30,
  "assert": { "expected_status": [200] }
}'
# -> {"run_id":"...","status":"running"}
```

Open `http://localhost:5173/runs/<run_id>` and watch correctness fall from 100% to near zero as offered RPS crosses ~150, while p99 latency stays flat at ~26 ms. That gap is the whole point: a latency monitor sees nothing wrong.

Prefer clicking? The dashboard has a **New run** form and a **Compare APIs** form. Point the target at your own URL instead of the mock whenever you like.

---

## Use it in your workflow

**Dashboard** - exploratory runs, the four visualizations (correctness-vs-load, latency histogram, time-series, comparison overlay), live over SSE.

**CLI** - download `bench` from the [latest release](../../releases/latest) (`bench-x86_64-unknown-linux-gnu`, `bench-aarch64-apple-darwin`, `bench-x86_64-apple-darwin`), or `cargo build --release -p bench-cli`.

```bash
export BENCH_CONTROL_URL=http://localhost:8000
export BENCH_API_KEY=sk-...          # sent as Bearer to the TARGET, never persisted

bench run -t 'http://mock:8080/api?mode=fast500&cliff_rps=150' -R 300 -d 30 \
          --expected-status 200 -n cliff-demo

# Ratchet against a baseline; exits non-zero on regression (for CI).
bench regression <run_id> --baseline <baseline_run_id> \
      --p99-delta-pct 10 --correctness-delta-pct 1
```

**CI gate (GitHub Actions)** - fail a PR when correctness or p99 regresses under load. Drop the [correctness-gate action](.github/actions/correctness-gate) into a workflow:

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

**Compare two APIs (vendor eval)** - fire both against one shared schedule window so the comparison is fair (same time, same conditions, each at the full RPS):

```bash
curl -sX POST localhost:8000/v1/comparisons -H 'content-type: application/json' -d '{
  "name": "vendorA vs vendorB", "target_rps": 200, "duration_s": 30,
  "target_a": { "url": "https://api.vendor-a.com/v1/...", "method": "GET" },
  "target_b": { "url": "https://api.vendor-b.com/v1/...", "method": "GET" },
  "assert": { "expected_status": [200] }
}'
# The compare view shows winner_by { latency_p99, correctness, cost_per_request, rate_limit_onset }.
```

**Agents (MCP)** - the same engine as agent-callable tools: `run_benchmark`, `get_results`, `compare_apis`, `regression_check`, `list_templates`, `create_template`, `run_template`.

---

## What it measures

- **Correctness as a load axis.** Pass rate and failures split by class - transport, status, size, JSON-schema, value, latency - each bucketed by the offered RPS the request was sent at. The cliff is in the data.
- **Coordinated-omission-correct latency.** Latency is measured from a request's *intended* send time, so a stalled client accrues latency to the requests that should have fired - the thing wrk2 gets right and most tools get wrong. Corrected and uncorrected histograms are both kept; their delta is the omission error.
- **429 as a first-class artifact.** Rate-limit responses are counted separately and excluded from the correctness score (they're *our* load artifact, never the API's failure), with the onset RPS surfaced and `Retry-After` honored.
- **Cost.** Declare `cost_per_request_usd`; runs are gated at creation and auto-aborted in flight if they would blow the budget.
- **Fleet honesty.** Load is split across a worker fleet and HDR-merged losslessly. If a worker dies mid-run, effective RPS drops honestly with a `WORKER_LOST` warning - no fabricated load. A hard abort tears the whole fleet down.

---

## Status

Phase 1 is shipped and gate-verified. Each gate is objective and runnable:

| gate | proves | status |
|------|--------|--------|
| **#1 wrk2 agreement** | corrected p50/p99 within ±5% of wrk2 | green - `gates/results/gate1.json` (containerized runner: `gates/gate1_containers.sh`) |
| **#2 oracle** | injected fail% == reported fail% | green - `gates/results/gate2.json` (`gates/gate2_oracle.sh`) |
| **#3 fleet merge** | two half-load workers ≈ one full-load run | green - `gates/results/gate3.json` (`gates/gate3_merge.sh`) |

The whole path works end to end: create a run → fleet fires → live SSE ticks → persisted history → completed. Abort propagates to workers; a killed worker degrades gracefully; a restarted coordinator re-learns its fleet within ~10s; concurrent A/B comparison; credential custody is canary-verified.

**Not in this build (out of Phase-1 scope):** OpenTelemetry tracing, centralized log/metrics dashboards (Jaeger/Grafana), and browser visual-regression. Logs are structured JSON to stdout (`docker compose logs <service>`). A Loki/Grafana stack is available behind `docker compose --profile observability up` but the apps are not currently OTLP-instrumented.

---

## How it's built

| path | language | what |
|------|----------|------|
| [`engine/`](engine/)   | Rust | Worker + coordinator. COO-correct scheduler (port of wrk2's `usec_to_next_send`), corrected/uncorrected HDR histograms, inline assertion tiers, gRPC fleet with lossless HDR merge, abort + heartbeat death-detection. Owns the hot path. |
| [`control/`](control/) | Go | REST + SSE, run lifecycle, credential custody, dispatch to the coordinator, cost guard, offload eval pool (JSON-schema / path / regex), Postgres + Timescale. |
| [`web/`](web/)         | TS/React | Vite + Tailwind. The four visualizations, live over SSE; new-run, compare, and templates flows. |
| [`mock/`](mock/)       | Rust | Load-dependent failure-injection target. Modes: `healthy`, `fast500`, `truncate`, `wrong_value`, `slow_ok`, `ratelimit`. The test oracle. |
| [`cli/`](cli/)         | Rust | `bench` - thin client of the control plane (`run`, `regression`). |
| [`mcp/`](mcp/)         | TS | MCP server: seven agent-callable tools over the control REST. |
| [`contracts/`](contracts/) | - | **Frozen truth**: `bench.proto` (worker↔coordinator), `api.md` (REST+SSE), `schema.sql` (Postgres+Timescale). Build against these. |
| [`gates/`](gates/)     | - | The three acceptance gates, their runners, and committed result artifacts. |

## Hard rules (architectural invariants)

1. Workers never persist and never authenticate - they fire load and stream results.
2. The control plane never generates load.
3. A slow DB write must never back-pressure the request scheduler.
4. Target API keys live in memory for the run only - never persisted, never logged, scrubbed from exports. Verified by the credential canary test.
5. Latency is measured from **intended** send time, not actual (gate #1's invariant).

## Design + contracts

- [`latency-bench-architecture.md`](latency-bench-architecture.md) - the design doc and thesis.
- [`contracts/`](contracts/) - the frozen interfaces every component builds against. If a contract feels wrong, open an issue; don't drift the implementation.
- [`gates/`](gates/) - the acceptance gates and what each verifies.

## License

Early-stage; no formal license yet. If you want to use a piece, open an issue.
