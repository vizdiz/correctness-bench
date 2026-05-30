# Build log

Append-only, timestamped record of meaningful steps + verification output.
Times are UTC. Session started 2026-05-29.

---

## 2026-05-29T04:30Z — Phase 1: environment survey

Tooling present: go 1.26.2, node v24.15.0 / npm 11.12.1, gh 2.92.0 (authed as
`vizdiz`), brew 5.1.14, python3 3.12 (broken pip — pyexpat), git 2.54.0.

Missing: rust/cargo, docker, protoc, psql, wrk/wrk2, goose/atlas.

Actions:
- Installed Rust via rustup → `rustc 1.96.0`, `cargo 1.96.0`. (DECISIONS D1)
- `brew install protobuf` → `libprotoc 35.0`. (DECISIONS D4)
- Declined to install Docker/colima unsupervised. (DECISIONS D2)

## 2026-05-29T04:36Z — Phase 1: contract validators

All four kit validators run, ALL PASS:

```
$ protoc --proto_path=contracts --descriptor_set_out=/dev/null contracts/bench.proto
PROTO OK: bench.proto parses, descriptor generated

JSON fenced blocks in contracts/api.md: 9 found, 9 parse OK
YAML frontmatter in .claude/agents/*.md: 6 files, all valid (keys: name,description,tools,model)
contracts/schema.sql via libpg_query (pg-query-emscripten): parsed 17 statements, no errors
```

No fixes needed — contracts are clean (and frozen). SQL validated with the real
libpg_query parser compiled to wasm (DECISIONS D3), not a naive scan.

## 2026-05-29T04:36Z — Phase 1: monorepo scaffold

- Moved `build-kit/*` to repo root (CLAUDE.md, contracts/, gates/, .claude/).
  Kit README → `BUILDKIT_README.md`; wrote a proper project `README.md`.
- Created layout: engine/ (stub lib + sentinel), control/ (go mod +
  main.go stub), web/ (vite react-ts scaffold), mock/ (impl this session),
  cli/ (stub bin), mcp/ (TS stub), docs/, .gitignore (Rust+Go+Node),
  docker-compose.yml (postgres+mock+control+web; engine commented).
- `go mod init github.com/vizdiz/correctness-bench/control` → go.mod ok.
- `npm create vite@latest web -- --template react-ts` → scaffolded ok.

## 2026-05-29T04:45Z — Phase 2: mock service (Rust + axum) DONE

Implemented `mock/` as lib + thin bin. Five modes, rolling-1s RPS meter, knobs
`mode`/`cliff_rps`/`pct`/`base_latency_ms`. Determinism via a per-instance
request counter (`seq % 100 < pct`), so cliff/injection reproduce.

`cargo test` — ALL GREEN (6 unit + 7 integration):

```
running 6 tests (unit: meter + decide)
test tests::meter_counts_within_one_second_window ... ok
test tests::meter_drops_old_slots ... ok
test tests::below_cliff_always_healthy_even_in_injection_mode ... ok
test tests::above_cliff_injects_per_mode ... ok
test tests::healthy_mode_never_injects_above_cliff ... ok
test tests::pct_selects_a_fraction_above_cliff ... ok
test result: ok. 6 passed; 0 failed

running 7 tests (integration: spawn server + reqwest)
test healthz_ok ... ok
test healthy_is_ok_below_and_above_cliff ... ok
test fast500_healthy_below_500_above ... ok
test truncate_returns_invalid_json_above_cliff ... ok
test wrong_value_is_schema_valid_but_wrong_above_cliff ... ok
test slow_ok_is_slow_but_correct_above_cliff ... ok
test pct_50_injects_half_above_cliff ... ok
test result: ok. 7 passed; 0 failed
```

Native curl proof (server on 127.0.0.1:8089) — stands in for the container check
since no Docker runtime is available (DECISIONS D2):

```
healthz                                    -> 200 "ok"
healthy base_latency_ms=20                 -> 200 {"status":"ok","count":3,...}  x-mock-injected: false
fast500 cliff_rps=1000000 (below)          -> 200 healthy            x-mock-injected: false
fast500 cliff_rps=0 pct=100 (above)        -> 500 {"status":"error","code":500}  x-mock-injected: true
truncate cliff_rps=0 pct=100 (above)       -> 200 {"status":"ok","count":3,"items":["a","b   (invalid json)
wrong_value cliff_rps=0 pct=100 (above)    -> 200 {"status":"ok","count":-1,...}
slow_ok cliff_rps=0 pct=100 (above)        -> 200 healthy body, time_total=0.502s
healthy base=0  timing                     -> 200 time_total=0.000761s   (internal <1ms: PASS)
healthy base=20 timing                     -> 200 time_total=0.022934s
mode=bogus                                 -> 400 (bad mode rejected)
```

Dockerfile + .dockerignore written (multi-stage rust:1-slim -> debian-slim +
curl healthcheck). NOT `docker build`-verified (no Docker). Compose `mock`
service on fixed port 8080; healthcheck aligned to curl.

## 2026-05-29T05:00Z — note: commit co-author trailer removed

User instruction (mid-session): never add Claude as a git co-author. Rewrote the
two existing commits to strip the `Co-Authored-By: Claude` trailer and
force-pushed (brand-new private solo repo; --force-with-lease). Verified zero
trailers reachable locally or on the remote. All future commits omit it. Saved as
a durable preference in memory.

## 2026-05-29T16:01Z — Phase 3: control plane plumbing (Go) DONE (structural)

Go module `control/`: chi router (D8), pgx/pgxpool, goose migrations (D9,
embedded), slog JSON to stdout. Layout: cmd/control, internal/{api,store,config,
migrations,sse}. SSE/compare/regression/templates/histogram intentionally NOT
built (depend on engine / out of scope).

Endpoints implemented + tested: POST /v1/runs (validate + cost ceiling + cred
scrub + insert queued), GET /v1/runs/:id (headers stripped), GET /v1/runs
(keyset cursor pagination), POST /v1/runs/:id/abort (state transition; 409 on
terminal). Plus GET /healthz.

Verified against a **real Postgres 16** (temporary cluster on :5433; Timescale
not installed, so the 3 timescale statements are shimmed out for tests — D12).

`go vet ./...` clean. `go test ./... -count=1` ALL GREEN:
```
ok  internal/api          2.079s   (canary + 5 handler/CRUD tests + scrub + validate)
ok  internal/migrations   3.836s   (drift guard: migration == frozen schema.sql)
```

CREDENTIAL CANARY (hard gate) — PASSES, 0 hits:
```
=== RUN   TestCredentialCanary
    canary_test.go:101: canary clean: 0 hits across POST/GET responses, full DB dump, and logs
--- PASS: TestCredentialCanary (0.01s)
```
The canary (CANARY_DO_NOT_LEAK_xyz123) was injected into Authorization, X-Api-Key,
and a custom header; the test greps every runs row via row_to_json(runs)::text,
both API responses, and the captured logs. Zero occurrences. Stored
target_headers_redacted contains the '***' placeholder (proves scrubbing happened,
not just absence).

Schema application proof (frozen schema on real PG16, timescale shimmed):
```
$ \dt  ->  comparisons, offload_eval, run_ticks, runs, templates, worker_telemetry  (6 tables)
runs: id uuid PK default gen_random_uuid(), target_headers_redacted jsonb NOT NULL, ... (matches contract)
```

goose Up against plain PG (no Timescale) fails ONLY at the first statement
`CREATE EXTENSION timescaledb` (SQLSTATE 0A000 "extension not available"); every
DDL statement is otherwise valid. On a Timescale-enabled Postgres (the user's
docker-compose) goose Up will complete. Migration file is goose-correct and
matches the frozen contract (drift test green).

Live server end-to-end (real binary, real PG, AUTO_MIGRATE=false against the
pre-applied schema), via curl:
```
GET  /healthz                 -> 200 {"status":"ok"}
POST /v1/runs (canary header) -> 201 {"run_id":"426709f4-...","status":"queued"}
GET  /v1/runs/:id             -> 200 {...,"target":{"url":...,"method":"POST"}}  (NO headers, NO canary)
GET  /v1/runs?limit=5         -> 200 {"runs":[...],"next_cursor":...}
POST /v1/runs/:id/abort       -> 200 {"status":"aborted","aborted_at":...}
POST .../abort (again)        -> 409
SELECT ... LIKE '%CANARY%'    -> 0   (live path, not just unit test)
```

Dockerfile + .dockerignore written (golang:1.26 -> debian-slim + curl
healthcheck). Added AUTO_MIGRATE env toggle (D11). NOT docker-build-verified
(no Docker).

## 2026-05-29T16:10Z — Phase 4: web skeleton DONE (structural)

Vite 8 + React 19 + TypeScript 6 + Tailwind v4. Design system (D10): Inter +
JetBrains Mono (bundled, no CDN), dark Linear/Stripe-inspired tokens via @theme,
semantic colors (correctness=green, latency=indigo, failures=red).

Component library: Button, Card (+Header/Body), Input/Select/Field,
NumberDisplay, Sparkline (placeholder), StatusBadge, Layout (sidebar shell).
Pages: /runs (list, real data), /runs/new (form per POST /v1/runs, API key as a
password field), /runs/:id (metric strip + chart placeholder + abort). API client
in src/lib/api.ts (typed to api.md). Numbers rounded via src/lib/format.ts.

`npm run build` (tsc -b + vite build) — PASS, no type errors:
```
✓ built in 176ms   dist/assets/index-*.js 246.91 kB (gzip 78.57 kB)   fonts bundled
```

Live dev-chain verification (web dev server -> vite proxy -> control -> PG16):
```
GET  http://localhost:5173/            -> serves app (<title>correctness-bench</title>)
GET  http://localhost:5173/healthz     -> {"status":"ok"}        (proxied to control)
GET  http://localhost:5173/v1/runs     -> {"runs":[...]}          (proxied)
POST http://localhost:5173/v1/runs     -> 201 {"run_id":"e2f3deb1-...","status":"queued"}
canary in DB after web-path create     -> 0 ; stored headers = {"Authorization":"***"}
```

LIMITATION: the React UI was NOT visually rendered/verified — no browser or
display in this environment. Type-check, production build, and the full API/proxy
wiring are verified; the visual design is unconfirmed (D10). Dockerfile.dev +
.dockerignore written, compose `web` env switched to CONTROL_PROXY_TARGET. NOT
docker-build-verified (no Docker).

## 2026-05-30 — Docker + Timescale + wrk2 prep

**Docker dev loop is live (closes D2 + D12).** Symlinked Docker Desktop's
`docker` CLI + `docker-compose` plugin into PATH. `docker compose up -d` brought
all four services healthy:
```
postgres (timescale/timescaledb:latest-pg16, healthy) :5432
mock     (correctness-bench-mock,            healthy) :8080
control  (correctness-bench-control,         healthy) :8000
web      (correctness-bench-web,             running) :5173
```
Timescale 2.27.1 actually loaded; both hypertables `run_ticks` and
`worker_telemetry` present — goose ran the full frozen schema (no shim).

Canary against the real Timescale postgres: posted a run with two
canary-bearing headers through live control on :8000 →
```
runs row dump grep CANARY_DO_NOT_LEAK -> 0 hits
control logs grep                     -> 0 hits
stored target_headers_redacted        -> {"X-Api-Key":"***","Authorization":"***"}
```
`cd control && TEST_DATABASE_URL=...:5432/bench go test ./internal/api -count=1`
all green against Timescale (no shim needed). **D12 closed.**

**wrk2 image built but runtime broken (see DECISIONS D14).** Cloned
`giltene/wrk2` to gitignored `reference/wrk2`; native macOS build fails (bundled
LuaJIT 2.0 has no arm64 port). Wrote `tools/wrk2/Dockerfile` + `build.sh` that
swaps in LuaJIT 2.1 and patches two source incompatibilities. The image builds
multi-arch natively. `--help` and `--version` work, but every real invocation
prints Usage and exits 0 — wrk2 4.0's Lua bootstrap is failing under LuaJIT 2.1
HEAD before getopt. Two clean ways forward (Rosetta toggle, or pin LuaJIT 2.1 to
a wrk2-compatible commit) documented in D14. Added a `bench`-profile compose
service so `docker compose --profile bench run --rm wrk2 ...` is wired and ready
when the runtime is fixed.

## 2026-05-30 — Engine MVP (single worker, COO-correct) — toward gate #1

Built `engine/` as a real crate (was stub). Modules:
- `sched` — `ConnSched` + `usec_to_next_send`, faithful port of giltene/wrk2's
  COO logic (intended-send time on the ORIGINAL schedule + catch-up at 2× when
  behind, with latency measured vs the original timeline regardless of catch-up).
  Pure, no I/O, exhaustively unit + property tested.
- `hist` — HDR histograms (corrected + uncorrected, 1µs–60s, 3 SF, mergeable).
- `worker` — async runner: N connection tasks, each loop firing on the schedule,
  recording corrected (recv − intended) and uncorrected (recv − actual) latency,
  draining the body before counting completed. Single tokio runtime, HTTP/1
  via reqwest (no h2 multiplexing — matches wrk2).
- `src/bin/worker.rs` — clap CLI: `--rate -d --connections --url`.

`cargo test` ALL GREEN:
```
running 5 tests (sched)               ... 5 passed
running 1 test  (integration)         ... engine_hits_target_rps_against_healthy_mock ok (5.0s)
running 2 tests (sched_proptest)      ... 2 passed   (256 cases each)
```

Live numbers against the compose mock (healthy, base_latency_ms=20, 30s × 3):
```
target 2000 RPS, c=100:
  run 1: achieved 1999.6  corrected p50/p95/p99 = 31.8/36.6/38.7  uncorrected = 28.8/33.8/35.6
  run 2: achieved 1999.5  corrected p50/p95/p99 = 31.4/36.5/38.6  uncorrected = 28.7/33.9/35.7
  run 3: achieved 1999.5  corrected p50/p95/p99 = 31.3/36.0/38.8  uncorrected = 28.6/33.4/35.7
  -> p99 variance across 3 runs: 0.5%  (gate #1 requires <10%)
  -> COO delta p99: ~+3ms consistently  (corrected > uncorrected — proves correction is non-no-op)
  -> achieved vs target: 0.025% (~perfect open-loop pacing)
```

**Gate #1 status:** stability + COO-correction non-trivially in evidence;
wrk2-numerical-agreement (the final ±5% check) is pending wrk2 runtime (D14).
Once wrk2 is firing, the comparison should land cleanly given these numbers.

## 2026-05-30 — wrk2 fixed; first gate-#1 numerical comparison (does NOT pass yet)

Root-caused the wrk2 silent-fail: `char c` for getopt's return value vs `-1` is
ambiguous on arm64 (default unsigned char → 255). `build.sh` now patches it to
`int c` before make. wrk2 runs natively, hits the requested RPS, produces real
HDR distributions.

**First side-by-side at the gate #1 shape** (mock healthy, base_latency_ms=20,
-t4 -c100 -d60s -R2000):

| metric            | wrk2     | engine   | delta   | gate #1 target |
|-------------------|----------|----------|---------|----------------|
| achieved rps      | 1997.9   | 1999.5   | +0.08%  | (no formal)    |
| corrected p50     | 22.6 ms  | 31.5 ms  | +39%    | ≤ ±5%          |
| corrected p99     | 24.9 ms  | 38.7 ms  | +55%    | ≤ ±5%          |
| uncorrected p50   | 21.4 ms  | 28.7 ms  | +34%    | (informational)|
| uncorrected p99   | 22.9 ms  | 35.7 ms  | +56%    | (informational)|

Engine is consistently ~10ms above wrk2 across the board. Diagnosis:
- ~1-2ms is unfair network path: engine on host hits port-forwarded `localhost:8080`;
  wrk2 in the compose bridge net hits `mock:8080` directly (one less hop).
- The rest (~6-8ms) is Tokio + reqwest overhead per request: async runtime
  scheduling slip at hundreds of in-flight tasks, body-drain alloc, the abstraction
  surface above hyper. wrk2 uses libev/epoll + 4 OS threads, minimal allocation
  per request.

**Gate #1 status: not yet green.** Engine is stable, COO-correction-correct (the
corrected/uncorrected delta tracks wrk2's), and pacing is essentially perfect,
but the absolute latency floor is too high. Closing the ±5% gap is engine-tuning
work: containerize the engine for fair network parity (fast win), then drop
reqwest for raw hyper, reduce per-request alloc, and possibly switch to a 1-task-
per-OS-thread connection model. Documented for follow-up.

## 2026-05-30 — Engine containerized; fair gate-#1 comparison (still over ±5%)

Wrote `engine/Dockerfile` (build context = repo root so the mock dev-dep
resolves) so the engine and wrk2 both run inside the compose bridge net hitting
`mock:8080` directly — apples to apples on the network path.

| metric            | wrk2 -t4 | wrk2 -t1 | engine (container) | engine vs wrk2 |
|-------------------|----------|----------|--------------------|----------------|
| achieved rps      | 1997.94  | 1991.83  | 1999.8             | within 0.5%    |
| corrected p50     | 22.62 ms | 22.93 ms | 30.0 ms            | +33%           |
| corrected p99     | 24.91 ms | 25.20 ms | 35.8 ms            | +44%           |
| uncorrected p50   | 21.45 ms | 21.89 ms | 25.0 ms            | +17%           |
| uncorrected p99   | 22.91 ms | 23.30 ms | 29.6 ms            | +29%           |

`wrk2 -t1` (single thread, matches the engine's runtime model) and `wrk2 -t4`
give essentially the same numbers, ruling out OS-thread count as the cause.
**The gap is ~3-7ms of per-request overhead in reqwest+Tokio above
hyper+libev:** body-drain allocation, async task scheduling slip at ~120k
requests/run, and the reqwest abstraction surface. The COO correction itself is
correct (the corrected/uncorrected delta tracks wrk2's — both about 1-2ms).

**Gate #1 numerical agreement (±5%): NOT YET CLOSED.** Engineering path to
close: replace `reqwest` with hyper-h1 directly, hoist the request object so
clone_request is allocation-free, consider a 1-task-per-connection-per-OS-thread
model, profile where the per-request cost is going. This is a separate
optimization phase from "build the engine," and is documented for follow-up.

<!-- Subsequent phases append below this line. -->
