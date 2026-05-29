# CLAUDE.md — orchestration guide

API correctness-under-load benchmarker. Monorepo. This file is the orchestrator's brief; component detail lives in each package's own CLAUDE.md and the design doc.

## Thesis (never lose this)
Measure **correctness as a continuous function of load**. The headline artifact is the cliff: latency flat while correctness collapses past some RPS. Every feature serves this or is out of scope. We are NOT a monitor (Datadog) and NOT just a load tester with checks (k6). Correctness is a measured axis, not a pass/fail gate.

## Hard rules (architectural invariants — never violate)
1. Workers never persist and never auth. Control plane never generates load.
2. A slow DB write must never back-pressure a request scheduler. (That manufactures coordinated omission in our own tool.)
3. API keys: in-memory for run duration only. Never persisted anywhere, never logged, scrubbed from traces, stripped from exports/shares.
4. Latency is measured from INTENDED send time, not actual. This is non-negotiable and is what gate #1 verifies.

## Frozen contracts (read-only truth — do not edit without explicit human sign-off)
- `contracts/bench.proto` — worker ↔ coordinator gRPC.
- `contracts/api.md` — control plane REST + SSE schema.
- `contracts/schema.sql` — Postgres + Timescale data model.
All agents build AGAINST these. If a contract feels wrong, STOP and flag the human; do not unilaterally change it.

## Packages (monorepo layout)
- `engine/`   — Rust. Worker + coordinator. COO scheduler, HDR, gRPC. Owns the hot path.
- `control/`  — Go. Lifecycle, REST/SSE, creds, offload eval pool.
- `web/`      — TS. UI + the 4 visualizations.
- `mock/`     — Rust. Load-dependent failure injection. The test bench.
- `cli/`      — Rust. CI on-ramp, pushes to dashboard.
- `mcp/`      — TS. Agent-facing tools.

## Build sequence (dependency order — mostly sequential early)
0. `mock/` first. Everything validates against it.
1. `engine/` single worker, COO-correct. **BLOCKS EVERYTHING until gate #1 green.**
2. `control/` + `web/` can parallelize once the engine streams real data.
3. coordinator + 2nd worker (fleet).
4. comparison + decision layer.
5. offload eval (expensive assertion tier).
6. `cli/`, `mcp/` last (thin).

## Gates = acceptance tests (objective, runnable — an agent is NOT done until its gate passes)
- `gates/gate1_wrk2_agreement.md` — engine's corrected p50/p95/p99 must match wrk2 on the mock, healthy mode.
- `gates/gate2_oracle.md` — inject known failure %; reported % must match within tolerance.
- `gates/gate3_merge.md` — two half-load workers ≈ one full-load run.

## Orchestration notes
- Prefer ONE agent on the critical path (engine) early. Parallel agents burn quota linearly; three blocked agents waste it.
- Use the built-in Explore agent (read-only, cheap) for codebase research before dispatching a builder.
- Freeze contracts BEFORE parallel work. Moving contracts = integration hell.
- `/clear` between unrelated dispatches to cut token cost.
- Each builder agent must run its gate before reporting done.

## Phasing
- **Phase 1 (~1.5 weeks, demoable v1).** Engine + fleet + control plane + single-page web + mock + CLI. Must pass gates #1, #2, #3. Logs are stdout-only. Cost handling via user-declared `cost_per_request_usd`. Property-based tests on the scheduler + differential matrix vs wrk2.
- **Phase 2 (after Phase 1 ships, or sooner if Phase 1 finishes early).** MCP server, OpenTelemetry, centralized log aggregation, screenshot regression on web, multi-page UI evolution.
- **Out of scope (do NOT build during Phase 1 even if asked):** continuous monitoring/alerting, geo probes, target-side resource metrics, payload-size sweeps, user auth, threshold gating / query language in CLI, OTel, Loki/ELK, Playwright. If an agent thinks any of these would be helpful, STOP and flag the human.

## Deployment
- Single cloud VM running the same `docker-compose.yml` as local. Services: control, coordinator, worker-1..N, mock, postgres (with Timescale extension).
- Worker discovery: dynamic. Workers POST `/workers/register` to the coordinator on startup; in-memory map; heartbeats handle liveness. No Consul/etcd/k8s.
- Default worker count: 4.
- Logging: JSON to stdout per service. `docker-compose logs` is the v1 debugging surface.
