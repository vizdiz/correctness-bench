# Gate proof artifacts

Generated, committable evidence that the three acceptance gates pass. Each gate
runner writes a `gateN.json` here when it runs:

| File | Produced by | Proves |
|------|-------------|--------|
| `gate1.json` | `gates/wrk2_sweep.sh` (Linux) or `gates/gate1_containers.sh` (macOS/Docker) | Engine's COO-corrected latency agrees with wrk2 across an RPS sweep (±5%). |
| `gate2.json` | `gates/gate2_oracle.sh` | Reported failure rate matches the injected rate per mode (±tolerance). |
| `gate3.json` | `gates/gate3_merge.sh` | Two half-load workers aggregate to one full-load run (HDR merge lossless, rate synced). |

Each artifact carries a top-level `verdict` (`PASS`/`FAIL`), a UTC `generated_at`
timestamp, the run `params`, and the per-check breakdown.

The committed `gate1.json` was generated via the containerized path (see below):
wrk2 emits p50/75/90/99 (not p95), so the comparison is on **p50 and p99** at the
gate's spec conditions (`base_latency_ms=20`, `c100`).

## How to regenerate (on an equipped host)

These require a real load environment — they are NOT runnable in a bare CI
container without the toolchain below.

```sh
# Gate 1, Linux native — needs the wrk2 binary (see tools/wrk2/) + cargo:
WRK2_BIN=tools/wrk2/wrk gates/wrk2_sweep.sh
# Gate 1, macOS/arm64 — native wrk2 does not build there (LuaJIT bytecode does
# not embed), so run wrk2 + engine + mock as containers on one docker network:
gates/gate1_containers.sh          # uses the prebuilt compose images

# Gate 2 needs cargo + python3:
gates/gate2_oracle.sh

# Gate 3 needs cargo + python3 (coordinator + worker_node + mock binaries):
gates/gate3_merge.sh
```

Commit the refreshed `gateN.json` alongside the change that motivated the re-run,
so the gate status travels with the code that has to satisfy it.
