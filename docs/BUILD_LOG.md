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

<!-- Subsequent phases append below this line. -->
