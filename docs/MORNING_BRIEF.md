# Morning brief — 2026-05-29 overnight session

**Repo:** https://github.com/vizdiz/correctness-bench (private)
**Branch:** `main` · 4 commits, all pushed, all green where verifiable.

TL;DR: Scaffold + mock + control plane + web skeleton are all DONE and verified
as far as the tooling allowed. The big asterisk: **this machine has no Docker and
no Timescale**, so nothing was run inside a container and the hypertable path
wasn't exercised. I verified everything else natively against a real Postgres 16
and a live control plane. **I did not touch the engine** (your supervised work).

---

## What's done (with commit SHAs)

| SHA | What | Verified |
|-----|------|----------|
| `2ffea0c` | Scaffold: monorepo, frozen contracts, gates, personas, docs, .gitignore, docker-compose | all 4 kit validators pass (protoc / JSON / agent YAML / schema.sql via libpg_query) |
| `edc4801` | **mock** — Rust+axum, all 5 modes, RPS meter, knobs, Dockerfile | `cargo test` 6 unit + 7 integration green; curl proof per mode; healthy <1ms internal |
| `be32e5d` | **control** — Go, chi+pgx+goose, 4 endpoints, credential canary | `go test` green incl. **canary 0 hits**; live server curl'd against real PG16 |
| `0f6b085` | **web** — Vite/React/TS/Tailwind v4, design system, 3 stub pages, run-create wired | `npm run build` type-checks; full web→proxy→control→PG chain curl'd |

Details + exact command output in `docs/BUILD_LOG.md`.

### Highlights
- **Credential canary passes with ZERO hits** — canary injected into 3 header
  positions, then grepped across a full `row_to_json(runs)` dump, both API
  responses, and logs. Stored header value is `"***"`. Verified again through the
  web proxy path.
- **mock cliff is real and deterministic** — `cliff_rps=0/1000000` pins the side
  of the cliff without timing games; `pct` selects an exact fraction.
- **Frozen contracts honored** — never edited `contracts/`. The goose migration is
  a verbatim copy of `schema.sql` with a drift test asserting they stay identical.

---

## What's NOT done and why

1. **Engine (worker + coordinator).** Untouched on purpose — supervised. This is
   the critical path; gate #1 blocks everything. `engine/` is a stub.
2. **No container verification.** Docker isn't installed and I declined to install
   a VM runtime (colima) unsupervised — too invasive while you slept (see
   DECISIONS **D2**). All Dockerfiles + `docker-compose.yml` are written but never
   `docker build` / `docker compose up`-run.
3. **Timescale not exercised.** No timescaledb on the box. The frozen schema's
   `CREATE EXTENSION timescaledb` + 2 `create_hypertable()` calls were shimmed out
   for tests (hypertables → plain tables; DECISIONS **D12**). goose Up against
   plain PG fails *only* at that extension line — everything else is valid DDL.
4. **Web UI not visually verified.** No browser/display here. Type-check, build,
   and API wiring are confirmed; the actual *look* is unconfirmed (DECISIONS D10).
5. **Deliberately skipped (engine-dependent / scope):** SSE stream, compare,
   regression-check, templates, histogram endpoint; CLI + MCP (stubs only).

---

## Needs your decision (see docs/DECISIONS.md)

- **D2** — Docker: install Docker Desktop, or `brew install colima docker
  docker-compose && colima start`? Needed for the real dev loop + Timescale.
- **D5** — Rust crates are independent (no top-level Cargo workspace). Fine?
- **D8 / D9** — router = **chi**, migrations = **goose**. OK to keep?
- **D11** — added an `AUTO_MIGRATE` env toggle (default true). Keep, or run
  migrations as a separate deploy job?
- **D12** — test-only Timescale shim. Re-run the suite against real Timescale once
  Docker is up to confirm the hypertable path (expected to pass unchanged).
- **D10** — web font (Inter + JetBrains Mono) + dark palette. Eyeball it.

Toolchains I installed (all reversible): **rustup** (rust 1.96), **protobuf**
(protoc 35), **postgresql@16** (binaries only). See D1/D3/D4.

Also: per your message I removed the `Co-Authored-By: Claude` trailer from the two
earlier commits (history rewritten + force-pushed on this fresh solo repo) and no
commit since includes it.

---

## Recommended first task tomorrow

**Stand up the real dev loop, then start the engine toward gate #1.**

1. Install Docker + `wrk2` (gate #1's oracle) + (optionally) confirm Timescale via
   the compose `postgres` image. Then `docker compose up` and confirm
   mock/control/postgres/web all come healthy. Re-run `cd control && TEST_DATABASE_URL=… go test ./...`
   against the **Timescale** DB to close the D12 gap.
2. Begin **E.1** (`docs/PLAN.md`): single Rust worker, COO scheduler (port
   `usec_to_next_send` from stock wrk2 — clone & read, don't modify), HDR
   corrected+uncorrected, open-loop. Then run `gates/gate1_wrk2_agreement.md`
   against the mock in healthy mode. **Nothing downstream matters until gate #1 is
   green.**

---

## Quick start (to reproduce what I ran)

```bash
# mock
cd mock && cargo test && cargo run            # :8080

# a throwaway Postgres (no Timescale) like I used:
PGBIN=/opt/homebrew/opt/postgresql@16/bin
$PGBIN/initdb -D /tmp/cb-pg -U postgres --auth=trust
$PGBIN/pg_ctl -D /tmp/cb-pg -o "-p 5433 -k /tmp" start
$PGBIN/createdb -h 127.0.0.1 -p 5433 -U postgres bench

# control tests (incl. canary) against it
cd control && TEST_DATABASE_URL="postgres://postgres@127.0.0.1:5433/bench?sslmode=disable" go test ./...

# web
cd web && npm install && npm run dev          # :5173, proxies /v1 -> :8000
```
