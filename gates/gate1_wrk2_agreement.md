# Gate 1 — wrk2 agreement (BLOCKS EVERYTHING)

The engine's coordinated-omission correction must produce the same numbers as wrk2. If it doesn't, the engine has a bug and nothing built on top of it matters.

## Setup
1. Start `mock/` in healthy mode (no injected failure), fixed base latency (e.g. 20ms): `GET /api?mode=healthy&base_latency_ms=20`.
2. Run wrk2 against it: `wrk -t4 -c100 -d60s -R2000 --latency http://mock/api`. Capture corrected p50/p95/p99.
3. Run `engine/` single worker, open-loop, same 2000 RPS, 60s, same target.

## Pass criteria
- engine corrected p50, p95, p99 each within ±5% of wrk2's.
- engine's uncorrected histogram differs from corrected (proves the correction is actually doing something, not a no-op).
- Re-run 3x; results stable (p99 variance < 10% across runs).

## Why ±5%
Different runtimes (C epoll vs Tokio) won't be bit-identical. >5% drift means a real scheduling or measurement bug, not noise.

## This gate exists because
Coordinated omission is the one thing wrk2 got right that everyone else gets wrong. If our numbers match wrk2's, the COO port is correct. This is THE checkpoint.

## Differential matrix sweep
`gates/wrk2_sweep.sh` extends the single-point gate to a small matrix
(RPS in {200, 500, 1000} by default; override with `RUNS=`). For each
(RPS, percentile in {p50, p95, p99}) pair the script asserts engine and wrk2
agree within ±5% (override with `TOLERANCE_PCT=`).

Prereqs:
- `cargo` on PATH (the script builds engine + mock in release mode).
- wrk2 binary at `tools/wrk2/wrk` or via `WRK2_BIN=/path/to/wrk`. Build with
  `docker build -t wrk2-local tools/wrk2/` then copy `/wrk2/wrk` out.

Run: `gates/wrk2_sweep.sh`. Exits 0 on full agreement, 1 on any drift, 2 on a
setup error (missing wrk2 binary, cargo missing, etc.).
