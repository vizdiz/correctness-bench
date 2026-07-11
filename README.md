# correctness-bench

**Measure correctness as a continuous function of load.**

The headline a benchmark should show isn't latency, it's the **cliff** where an
API stops answering correctly while latency stays flat. wrk2 doesn't see it
(fast 500s count as wins). Postman doesn't see it (one request). k6 with checks
sees pass/fail gates, not the cliff *curve*.

This shows the cliff live, on one screen, in one run:

```
  Correctness vs offered RPS
   100% ─●●●●●●●●●●●●●●●●●─╮
                            ●─────●─●─●─●─●─●─●─●─●─●─●─●─●─●     ← correctness (green)
                            │
     0%                     ╰──────────────────────────────────
                          150 rps (cliff)             →  offered RPS

   Latency p99 (right axis)
    ~26ms ───────────────────────────────────────────────────    ← latency stays flat (blue)
```

What this is. The thing being benchmarked could be a SaaS API, an LLM endpoint,
your service in staging - anything HTTP. You point it at the URL, declare what
"correct" means (status, schema, value, max latency), and watch correctness as
load climbs. You learn the cliff RPS, not just the latency distribution.

What this isn't. Not a monitor (Datadog passively watches; this actively probes).
Not just k6 with checks (correctness is a measured AXIS, not a pass/fail gate).
Not a target-side metrics tool (this is outside-in, pre-adoption).

## Status

Phase 1 and Phase 2 are both in place. Gates the design doc requires:

| gate | what | status |
|------|------|--------|
| **#1 wrk2 agreement** | engine corrected p50/p95/p99 within ±5% of wrk2 | green; `gates/wrk2_sweep.sh` extends the gate to an RPS matrix |
| **#2 oracle** | inject known fail %; reported % matches within tolerance | green across `fast500`, `slow_ok`, `truncate`; runner at `gates/gate2_oracle.sh` |
| **#3 fleet merge** | two half-load workers ≈ one full-load run | green; HDR merged losslessly in coordinator, in-tree test `dispatch_gate3_half_plus_half_equals_full` (run with `--ignored`) |

Phase 1 surface area:

| component | status |
|-----------|--------|
| `mock` - Rust+axum target with 5 load-dependent failure modes | shipped |
| `engine` - Rust worker: COO-correct scheduler, HDR histos, raw TCP + httparse, inline assertions | shipped |
| `engine` - fleet: coordinator + worker_node gRPC (per `bench.proto`), HDR merge across workers | shipped |
| `control` - Go: REST + SSE, run lifecycle, credential canary, compare / regression-check / templates / histogram endpoints | shipped |
| `web` - Vite/React/Tailwind: cliff / histogram / time-series / compare views, templates page, runs picker, status + name filters | shipped |
| `cli` - `bench run` for live runs, `bench regression --baseline <id>` for ratchets | shipped |
| `mcp` - agent-callable tools: `run_benchmark`, `get_results`, `compare_apis`, `regression_check`, `list_templates`, `create_template`, `run_template` | shipped |
| offload eval pool (JSON Schema / JSON-path / regex tiers on sampled bodies) | shipped |

Phase 2 surface area:

| component | status |
|-----------|--------|
| OpenTelemetry: traces + metrics across engine, control, coordinator, worker_node. W3C tracecontext propagated end-to-end (HTTP + gRPC). Auto-instrumented via `otelhttp` + `otelpgx` on control. Engine emits per-tick request counters and a scheduler-slip histogram. | shipped |
| Centralized logs: JSON-structured logs from every service, scraped by Promtail (Docker socket SD + JSON parse pipeline) and shipped to Loki | shipped |
| Observability backends: OTel collector fanning out to Jaeger (traces), Loki (logs), Prometheus (metrics). Grafana auto-provisioned with all three datasources + Loki↔Jaeger trace-id linking | shipped |
| Screenshot regression: Playwright (Chromium), hermetic via `page.route` fixtures, 8 visual specs across runs list, run detail (cliff / histogram / time), compare, templates | shipped |
| CI: GitHub Actions per-package (engine, control, cli, mock, mcp, web), web visual regression upload-on-failure | shipped |

## Quick start

```bash
# 1. Bring up the whole stack: postgres+timescale, mock, control, web, coordinator,
#    2x worker_node, otel-collector, jaeger, loki, promtail, prometheus, grafana.
docker compose up -d

# 2. Open the surfaces:
#    web UI ............ http://localhost:5173
#    Jaeger ............ http://localhost:16686
#    Grafana ........... http://localhost:3000   (auto-login, anon admin)
#    Prometheus ........ http://localhost:9090
#    control healthz ... http://localhost:8000/healthz
#    coordinator admin . http://localhost:9091
```

### Fire a run via the coordinator (fleet path)

```bash
RUN_ID=$(curl -s -X POST http://localhost:8000/v1/runs \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "cliff-demo",
    "target": {"url": "http://mock:8080/api?mode=fast500&cliff_rps=150&pct=100&base_latency_ms=20",
               "method": "GET", "timeout_ms": 5000, "verify_tls": true},
    "target_rps": 300, "duration_s": 30, "load_model": "open", "connections": 30,
    "assert": {"expected_status": [200]},
    "rate_limit_policy": {"action": "backoff", "record_onset": true}
  }' | python3 -c "import sys,json; print(json.load(sys.stdin)['run_id'])")

curl -X POST http://localhost:9091/admin/runs \
  -H 'Content-Type: application/json' \
  -d "{
    \"run_id\": \"$RUN_ID\",
    \"target_url\": \"http://mock:8080/api?mode=fast500&cliff_rps=150&pct=100&base_latency_ms=20\",
    \"target_method\": \"GET\",
    \"target_rps\": 300, \"duration_s\": 30, \"connections\": 30, \"keepalive\": true,
    \"timeout_ms\": 5000, \"expected_status\": [200],
    \"control_tick_url\": \"http://control:8000/v1/_internal/runs/$RUN_ID/tick\",
    \"control_finalize_url\": \"http://control:8000/v1/_internal/runs/$RUN_ID/finalize\"
  }"

# Open http://localhost:5173/runs/$RUN_ID to watch the cliff form live, then
# check Jaeger at http://localhost:16686 for the end-to-end trace that ties
# coordinator -> worker_node -> control -> postgres into one tree.
```

### Or use the CLI

```bash
# Create + stream a run.
cargo run -p bench-cli -- run \
  --target 'http://localhost:8080/api?mode=fast500&cliff_rps=150&pct=100&base_latency_ms=20' \
  --rps 300 --duration-s 30 --expected-status 200 --name cliff-demo

# Regression check a new run against a finalized baseline.
cargo run -p bench-cli -- regression $RUN_ID \
  --baseline $BASELINE_RUN_ID \
  --p99-delta-pct 10 --correctness-delta-pct 1
```

## Control plane endpoints (`contracts/api.md`)

| method | path | what |
|--------|------|------|
| `POST`   | `/v1/runs` | create a queued run (target.headers scrubbed before persistence) |
| `GET`    | `/v1/runs` | paginated list, supports `?status=` filter and cursor |
| `GET`    | `/v1/runs/:id` | run view + `final` percentiles + `offload` counts + `cost_per_request_usd` |
| `POST`   | `/v1/runs/:id/abort` | terminate |
| `GET`    | `/v1/runs/:id/stream` | SSE: live tick events |
| `GET`    | `/v1/runs/:id/histogram` | V2-deflate corrected HDR (raw bytes), or `?format=json` for binned JSON; `?which=uncorrected` for the COO-delta |
| `GET`    | `/v1/runs/:id/compare/:id2` | side-by-side + winner_by axes + fairness flags |
| `POST`   | `/v1/runs/:id/regression-check` | pass/fail vs baseline with `{p99_delta_pct, correctness_delta_pct}` thresholds |
| `POST`   | `/v1/templates` | save a redacted run spec |
| `GET`    | `/v1/templates` | list |
| `POST`   | `/v1/templates/:id/run` | fork a template into a fresh run (re-supplies redacted headers) |

Internal endpoints (engine + coordinator only):

| method | path | what |
|--------|------|------|
| `POST` | `/v1/_internal/runs/:id/tick` | live tick ingest (broker forwards to SSE subscribers, offload pool evaluates `sampled` bodies) |
| `POST` | `/v1/_internal/runs/:id/finalize` | one-shot finals: percentiles, totals, cliff_rps, base64 V2-deflate HDRs, status transition |

## Repo layout

| path | language | what |
|------|----------|------|
| [`engine/`](engine/)   | Rust | Worker. COO scheduler (port of wrk2's `usec_to_next_send`), HDR histograms (corrected + uncorrected), inline assertions (status/size/latency/content-type), gRPC coordinator + multi-worker fleet, OTel traces + metrics. |
| [`control/`](control/) | Go   | REST + SSE per [`contracts/api.md`](contracts/api.md). Lifecycle, credential custody, in-memory tick broker, postgres+timescale persistence, offload eval pool (JSON-Schema / JSON-path / regex), OTel via `otelhttp` + `otelpgx`. |
| [`web/`](web/)         | TS/React | Vite + Tailwind v4. Cliff, histogram (HDR server-binned), time-series, compare, templates pages. Playwright visual regression. |
| [`mock/`](mock/)       | Rust | Load-dependent failure-injection target. 5 modes: `healthy`, `fast500`, `truncate`, `wrong_value`, `slow_ok`. Knobs `mode/cliff_rps/pct/base_latency_ms`. |
| [`cli/`](cli/)         | Rust | `bench` thin client of the control plane (`run` + `regression` subcommands). |
| [`mcp/`](mcp/)         | TS   | MCP server: 7 agent-callable tools wrapping the control REST. |
| [`contracts/`](contracts/) | - | **Frozen**: `bench.proto` (worker↔coordinator gRPC), `api.md` (REST+SSE), `schema.sql` (Postgres+Timescale). |
| [`gates/`](gates/)     | - | Acceptance gates + runner scripts: `wrk2_sweep.sh` (#1 matrix), `gate2_oracle.sh` (#2 oracle), gate #3 in-tree as `cargo test --ignored`. |
| [`tools/wrk2/`](tools/wrk2/) | - | Dockerfile that builds wrk2 native-arch (works around bundled-LuaJIT-2.0 + arm64 + the `char c` getopt bug). |
| [`ops/`](ops/)         | yaml | otel-collector / loki / promtail / prometheus / grafana configs consumed by `docker-compose.yml`. |
| [`.github/workflows/`](.github/workflows/) | yaml | per-package CI (engine, control, cli, mock, mcp, web + Playwright). |

## Hard rules (architectural invariants)

1. Workers never persist and never auth - they fire load and stream results.
2. Control plane never generates load.
3. A slow DB write must not back-pressure the request scheduler.
4. Target API keys: in memory for the run only, never persisted, scrubbed
   from logs/exports. Verified by the credential canary test.
5. Latency is measured from **intended** send time (gate #1's invariant).

## Observability

`docker compose up` starts the full stack including telemetry:

- Each app service (`engine-worker`, `coordinator`, `worker_node`, `control`)
  emits OTLP to `otel-collector:4317`. The collector fans out to Jaeger
  (traces), Loki (structured logs), and a Prometheus exporter on `:8889`
  scraped by the Prometheus server.
- W3C tracecontext is propagated: tonic interceptor on coordinator→worker_node
  gRPC, reqwest header injection on tick/finalize POSTs, `otelhttp` middleware
  on the control side. Jaeger shows one trace per run spanning every service.
- `service_name` is promoted to a Prometheus label so series filter cleanly
  per service. Engine ships `engine.requests{outcome=...}` and
  `engine.scheduler_slip_us`. Control gets `http.server.request.duration`,
  `db.client.operation.duration`, request/response body size automatically.
- Grafana on `:3000` is auto-provisioned with all three datasources and a
  Loki→Jaeger trace-id link so clicking on a `trace_id` in a log line jumps
  to the matching trace.

## CI + visual regression

- `.github/workflows/ci.yml` runs per-package on every push and PR: engine
  (`cargo test`), control (`go test -race`), cli (`cargo build`), mock
  (`cargo test`), mcp (`tsc --noEmit`), web (`vite build` + Playwright).
- `web/tests/visual/` holds 8 hermetic Playwright specs. Network is mocked
  via `page.route` so the suite runs without docker / postgres / engine and
  stays pixel-stable. Update baselines with `npm run test:visual:update`.

## Try the cliff yourself

The single most pitch-y experiment, in compose:

```bash
docker compose up -d

RUN_ID=$(curl -s -X POST http://localhost:8000/v1/runs \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "ramp",
    "target": {"url":"http://mock:8080/api?mode=fast500&cliff_rps=150&pct=100&base_latency_ms=20","method":"GET","timeout_ms":5000,"verify_tls":true},
    "target_rps": 300, "duration_s": 30, "load_model": "open", "connections": 30,
    "assert": {"expected_status":[200]},
    "rate_limit_policy": {"action":"backoff","record_onset":true}
  }' | python3 -c "import sys,json; print(json.load(sys.stdin)['run_id'])")

curl -X POST http://localhost:9091/admin/runs -H 'Content-Type: application/json' \
  -d "{\"run_id\":\"$RUN_ID\",\"target_url\":\"http://mock:8080/api?mode=fast500&cliff_rps=150&pct=100&base_latency_ms=20\",\"target_method\":\"GET\",\"target_rps\":300,\"duration_s\":30,\"connections\":30,\"keepalive\":true,\"timeout_ms\":5000,\"expected_status\":[200],\"control_tick_url\":\"http://control:8000/v1/_internal/runs/$RUN_ID/tick\",\"control_finalize_url\":\"http://control:8000/v1/_internal/runs/$RUN_ID/finalize\"}"

# Open http://localhost:5173/runs/$RUN_ID for the live cliff,
#      http://localhost:16686 for the end-to-end trace,
#      http://localhost:3000 for Grafana + Loki logs filtered by service.
```

The web dashboard draws the cliff live: correctness goes 100% → 0% as offered
RPS crosses 150, while latency p99 stays at ~26 ms across the whole sweep.

## Design + decisions

- [`latency-bench-architecture.md`](latency-bench-architecture.md) - the design doc / thesis.
- [`contracts/`](contracts/) - the frozen interfaces. The contracts (proto, REST/SSE, schema) are what every component builds against; if a contract feels wrong, file an issue, don't drift the implementation.
- [`gates/`](gates/) - the three acceptance gates and what they verify.
- [`docs/DECISIONS.md`](docs/DECISIONS.md) - non-obvious choices made along the way.
- [`docs/BUILD_LOG.md`](docs/BUILD_LOG.md) - append-only record of meaningful steps with verification output.

## License + contributions

Early-stage; no formal license yet. If you want to use a piece, open an issue.
