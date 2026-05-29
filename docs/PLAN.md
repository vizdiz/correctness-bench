# Phase 1 plan — demoable v1

Goal: prove the thesis (correctness as a continuous function of load; the
cliff). Must pass gates #1, #2, #3. Tasks are ~2-hour units, ordered by
dependency. "Owner" = the agent in `.claude/agents/` that drives it. "Gate" =
the objective acceptance test that closes it.

Legend: `[x]` done · `[~]` in progress · `[ ]` not started · `[S]` supervised (human in loop)

---

## Track 0 — scaffolding (this overnight session)

| # | Task | Owner | Closes when | DoD |
|---|------|-------|-------------|-----|
| 0.1 [x] | Monorepo scaffold, contracts in place, validators green | orchestrator | all 4 kit validators pass | repo builds, validators pass, pushed |
| 0.2 [x] | Mock service, 5 modes, tests, dockerized | mock | `cargo test` green + curl per mode | see Track 1 |
| 0.3 [x] | Control plane plumbing (4 endpoints + canary) | control | canary test = 0 hits, handler tests green | see Track 2 |
| 0.4 [x] | Web skeleton + design system | web | build passes, run-create wired (UI not visually verified) | see Track 3 |

---

## Track 1 — mock (DONE this session; everything validates against it)

| # | Task | Owner | Closes when | DoD |
|---|------|-------|-------------|-----|
| 1.1 | axum service skeleton + `/healthz` + rolling-1s RPS meter | mock | unit test on meter | meter reports rolling RPS within ±1 |
| 1.2 | 5 modes behind `mode`/`cliff_rps`/`pct`/`base_latency_ms` knobs | mock | per-mode integration tests | healthy <1ms internal + base; cliffs reproduce |
| 1.3 | Dockerfile + compose entry on fixed port 8080 | mock | `docker compose up mock` + curl | [S] pending Docker (see D2) |

## Track 2 — control plane plumbing (THIS session, structural only)

| # | Task | Owner | Closes when | DoD |
|---|------|-------|-------------|-----|
| 2.1 | go mod, chi router, pgx pool, goose migrations | control | `goose up` applies schema.sql clean | migration output captured |
| 2.2 | POST /v1/runs (validate, cost ceiling, insert, 201 queued) | control | handler test | spec validated, headers scrubbed pre-insert |
| 2.3 | GET /v1/runs/:id (shape per api.md, headers stripped) | control | handler test | no `target.headers` in response |
| 2.4 | GET /v1/runs (cursor pagination) | control | handler test | stable cursor, no dupes |
| 2.5 | POST /v1/runs/:id/abort (state transition only) | control | handler test | 409 on terminal; worker propagation = [S] engine |
| 2.6 | Credential canary test (CI) | control | grep finds 0 hits in DB + logs | **hard gate — never weaken** |

## Track 3 — web skeleton (THIS session if time/budget)

| # | Task | Owner | Closes when | DoD |
|---|------|-------|-------------|-----|
| 3.1 | vite+react+ts+tailwind, design system (font, palette, components) | web | components render in Storybook-less demo page | owned look, not generic AI |
| 3.2 | Stub pages /runs, /runs/new (form), /runs/:id (chart placeholder) | web | run-create POSTs to control | real run_id returned |

---

## Supervised — NOT this session (needs human; the riskiest, contract-critical code)

| # | Task | Owner | Gate |
|---|------|-------|------|
| E.1 [S] | Single worker: COO scheduler (port `usec_to_next_send`), HDR corrected+uncorrected, open-loop | engine | **gate #1 — BLOCKS ALL** |
| E.2 [S] | Inline assertions (transport/status/latency/size), RPS-bucketed | engine | gate #2 (status tier) |
| E.3 [S] | Coordinator: split, epoch-sync, lossless HDR merge; 2nd worker | engine | **gate #3** |
| E.4 [S] | Property tests on COO scheduler + differential matrix vs wrk2 | engine | matrix cells within ±5% |
| C.1 | Offload eval pool (schema/path/regex tiers, off hot path) | control | gate #2 variants (truncate/wrong_value) |
| C.2 | SSE live stream (tick/status/done) | control | live data flows to web |
| C.3 | Rate-limit detection (Retry-After / RateLimit-*; onset RPS) | control | 429 separated from correctness |
| W.1 | The four viz (headline cliff, histogram, time-series, comparison) | web | headline cliff legible at a glance |
| L.1 | CLI: run a bench, stream, dashboard link, exit codes | cli | end-to-end against control |
| M.1 | MCP server: run/get-results/compare/regression-check tools | mcp | tools map to api.md endpoints |

---

## Recommended first task tomorrow (awake)
**E.1 — single worker + COO scheduler + gate #1.** It blocks everything. Clone
stock wrk2, read `src/wrk.c` for `usec_to_next_send`, port it, run gate #1
against the mock in healthy mode. Nothing downstream matters until it's green.
Needs wrk2 + Docker installed first (neither present tonight — see DECISIONS D2).
