# correctness-bench

**API correctness-under-load benchmarker.** Measures *correctness as a
continuous function of load* — the headline artifact is the **cliff**: latency
stays flat while correctness collapses past some RPS. wrk2 counts a fast 500 as
a win; Postman sends one request; this tool catches the cliff.

> Not a monitor (Datadog). Not just a load tester with checks (k6). Correctness
> is a measured axis, not a pass/fail gate. See `latency-bench-architecture.md`.

## Layout (monorepo)

| Path | Lang | What |
|------|------|------|
| `engine/`  | Rust | Worker + coordinator. COO scheduler, HDR, gRPC, fleet. The hot path. **(supervised)** |
| `control/` | Go   | Lifecycle, REST + SSE, credential custody, offload eval pool. |
| `web/`     | TS   | UI + the four visualizations. Live via SSE. |
| `mock/`    | Rust | Load-dependent failure injection. The test bench. |
| `cli/`     | Rust | Thin client of the control plane. *(stub)* |
| `mcp/`     | TS   | Agent-facing tools. *(stub)* |
| `contracts/` | — | **Frozen**: `bench.proto`, `api.md`, `schema.sql`. |
| `gates/`   | — | Acceptance tests #1–#3. An agent isn't done until its gate is green. |

## Hard rules (architectural invariants)

1. Workers never persist and never auth. The control plane never generates load.
2. A slow DB write must never back-pressure the request scheduler.
3. Target API keys: in memory for the run only — never persisted, never logged,
   stripped from exports. Verified by the credential canary test.
4. Latency is measured from **intended** send time, not actual. Gate #1 verifies it.

## Gates

- **#1 wrk2 agreement** (`gates/gate1_wrk2_agreement.md`) — engine corrected
  p50/p95/p99 match wrk2 on the mock, healthy mode. **Blocks everything.**
- **#2 oracle** (`gates/gate2_oracle.md`) — inject a known failure %, report it within ±2%.
- **#3 merge** (`gates/gate3_merge.md`) — two half-load workers ≈ one full-load run.

## Dev loop

```bash
docker compose up                 # postgres (Timescale) + mock + control + web
docker compose up mock            # just the bench target on :8080
docker compose logs control       # JSON logs to stdout per service (v1 debugging surface)
```

Per-component dev without Docker:

```bash
# mock (Rust)
cd mock && cargo run                       # serves on 127.0.0.1:8080
cd mock && cargo test                      # per-mode integration tests

# control (Go) — needs a Postgres+Timescale reachable via $DATABASE_URL
cd control && go test ./...                # handler + canary tests
cd control && go run ./cmd/control         # serves on :8000

# web (TS)
cd web && npm install && npm run dev       # vite dev server on :5173
```

## Status

See `docs/PLAN.md` (task board), `docs/BUILD_LOG.md` (what's been verified),
`docs/DECISIONS.md` (choices flagged for review), and `docs/MORNING_BRIEF.md`
(latest session summary). Build kit usage notes preserved in `BUILDKIT_README.md`.
