# correctness-bench

**Measure correctness as a continuous function of load.**

The headline a benchmark should show isn't latency — it's the **cliff** where an
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
your service in staging — anything HTTP. You point it at the URL, declare what
"correct" means (status, schema, value, max latency), and watch correctness as
load climbs. You learn the cliff RPS, not just the latency distribution.

What this isn't. Not a monitor (Datadog passively watches; this actively probes).
Not just k6 with checks (correctness is a measured AXIS, not a pass/fail gate).
Not a target-side metrics tool (this is outside-in, pre-adoption).

## Status

Phase 1 (demoable v1) is mostly in place. Gates the design doc requires:

| gate | what | status |
|------|------|--------|
| **#1 wrk2 agreement** | Engine's corrected p50/p95/p99 within ±5% of wrk2 on the mock | ✅ green (p50 +2.6%, p99 −0.8%, 1.2% variance across runs) |
| **#2 oracle** | Inject known fail %; reported % matches within ±2% | ✅ green for the inline tiers (status, size, latency, content-type — exact match) |
| **#3 fleet merge** | Two half-load workers ≈ one full-load run | 🚧 coordinator in progress |

Phase 1 surface area:

| component | status |
|-----------|--------|
| `mock` — Rust+axum target with 5 load-dependent failure modes | ✅ |
| `engine` — Rust worker: COO-correct scheduler, HDR histos, raw TCP + httparse, inline assertions | ✅ |
| `control` — Go: REST + SSE, run lifecycle, credential canary (0 hits) | ✅ |
| `web` — Vite/React/Tailwind: live SSE, headline correctness-vs-load chart with latency on the right axis | ✅ |
| `cli` — `bench` thin client with live SSE dashboard | ✅ |
| `mcp` — agent-callable tools over the control REST | ✅ |
| `engine` — fleet: coordinator + multi-worker (`bench.proto` gRPC) | 🚧 |
| offload eval pool (JSON Schema / JSON-path / regex tiers on sampled bodies) | not yet |

## Quick start

```bash
# Bring up the stack: postgres+timescale, mock target, control plane, web UI.
docker compose up -d

# Open http://localhost:5173 — the web UI (Runs list, New run, live cliff chart).

# Or use the CLI to create a run + watch live ticks.
cargo run -p bench-cli -- \
  --target 'http://host.docker.internal:8080/api?mode=fast500&cliff_rps=100&pct=50&base_latency_ms=20' \
  --rps 200 --duration-s 10 --expected-status 200 --name demo
# Then in another terminal fire the engine against that run id:
docker run --rm --network correctness-bench_bench correctness-bench-engine \
  --url 'http://mock:8080/api?mode=fast500&cliff_rps=100&pct=50&base_latency_ms=20' \
  --rate 200 --duration-s 10 --connections 20 --expected-status 200 \
  --push-to http://control:8000 --run-id <RUN_ID_FROM_CLI>
```

(The CLI/MCP→engine link will close once the coordinator lands. Until then,
fire the engine separately.)

## Repo layout

| path | language | what |
|------|----------|------|
| [`engine/`](engine/)   | Rust | Worker. COO scheduler (port of wrk2's `usec_to_next_send`), HDR histograms, inline assertions (status/size/latency/content-type), raw-TCP request loop, gRPC coordinator + multi-worker fleet (in progress). |
| [`control/`](control/) | Go   | REST + SSE per [`contracts/api.md`](contracts/api.md). Lifecycle, credential custody, in-memory tick broker, postgres+timescale persistence. |
| [`web/`](web/)         | TS/React | Vite + Tailwind v4. Live SSE, headline correctness-vs-load chart. |
| [`mock/`](mock/)       | Rust | Load-dependent failure-injection target. 5 modes: `healthy`, `fast500`, `truncate`, `wrong_value`, `slow_ok`. Knobs `mode/cliff_rps/pct/base_latency_ms`. |
| [`cli/`](cli/)         | Rust | `bench` thin client of the control plane. |
| [`mcp/`](mcp/)         | TS   | MCP server: `run_benchmark`, `get_results`, `compare_apis`, `regression_check`. |
| [`contracts/`](contracts/) | — | **Frozen**: `bench.proto` (worker↔coordinator gRPC), `api.md` (REST+SSE), `schema.sql` (Postgres+Timescale). |
| [`gates/`](gates/)     | — | The three acceptance gates (#1, #2, #3). |
| [`tools/wrk2/`](tools/wrk2/) | — | Dockerfile that builds wrk2 native-arch (works around bundled-LuaJIT-2.0 + arm64 + the `char c` getopt bug). |
| [`latency-bench-architecture.md`](latency-bench-architecture.md) | — | The full architecture / thesis. |

## Hard rules (architectural invariants)

1. Workers never persist and never auth — they fire load and stream results.
2. Control plane never generates load.
3. A slow DB write must not back-pressure the request scheduler.
4. Target API keys: in memory for the run only, never persisted, scrubbed
   from logs/exports. Verified by the credential canary test.
5. Latency is measured from **intended** send time (gate #1's invariant).

## Try the cliff yourself

The single most pitch-y experiment, in compose:

```bash
# 1. Spin everything.
docker compose up -d

# 2. Create a run that ramps 0 → 300 rps across a cliff at 150 (mock fast500 pct=100).
RUN_ID=$(curl -s -X POST http://localhost:8000/v1/runs \
  -H 'Content-Type: application/json' \
  -d '{"name":"ramp","target":{"url":"http://mock:8080/api?mode=fast500&cliff_rps=150&pct=100&base_latency_ms=20","method":"GET","headers":{}},"target_rps":300,"duration_s":30,"load_model":"open","connections":30,"keepalive":true,"assert":{"expected_status":[200]},"rate_limit_policy":{"action":"backoff"}}' \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['run_id'])")

# 3. Open http://localhost:5173/runs/$RUN_ID

# 4. Fire the engine against it (ramping).
docker run --rm --network correctness-bench_bench correctness-bench-engine \
  --url 'http://mock:8080/api?mode=fast500&cliff_rps=150&pct=100&base_latency_ms=20' \
  --rate 300 --duration-s 30 --connections 30 --expected-status 200 --ramp \
  --push-to http://control:8000 --run-id $RUN_ID
```

The web dashboard will draw the cliff live: correctness goes 100% → 0% as
offered RPS crosses 150, while latency p99 stays at ~26 ms across the whole
sweep.

## Design + decisions

- [`latency-bench-architecture.md`](latency-bench-architecture.md) — the design doc / thesis.
- [`contracts/`](contracts/) — the frozen interfaces. The contracts (proto, REST/SSE, schema) are what every component builds against; if a contract feels wrong, file an issue, don't drift the implementation.
- [`gates/`](gates/) — the three acceptance gates and what they verify.
- [`docs/DECISIONS.md`](docs/DECISIONS.md) — non-obvious choices made along the way (router lib, migration tool, why we ship a custom wrk2 image, etc.).
- [`docs/BUILD_LOG.md`](docs/BUILD_LOG.md) — append-only record of meaningful steps with the verification output.

## License + contributions

Early-stage; no formal license yet. If you want to use a piece, open an issue.
