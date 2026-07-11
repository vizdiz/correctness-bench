# Gate proof artifacts

Generated, committable evidence that the three acceptance gates pass. Each gate
runner writes a `gateN.json` here when it runs:

| File | Produced by | Proves |
|------|-------------|--------|
| `gate1.json` | `gates/wrk2_sweep.sh` | Engine's COO-corrected p50/p95/p99 agree with wrk2 across an RPS sweep (±5%). |
| `gate2.json` | `gates/gate2_oracle.sh` | Reported failure rate matches the injected rate per mode (±tolerance). |
| `gate3.json` | `gates/gate3_merge.sh` | Two half-load workers aggregate to one full-load run (HDR merge lossless, rate synced). |

Each artifact carries a top-level `verdict` (`PASS`/`FAIL`), a UTC `generated_at`
timestamp, the run `params`, and the per-check breakdown.

## How to regenerate (on an equipped host)

These require a real load environment — they are NOT runnable in a bare CI
container without the toolchain below.

```sh
# Gate 1 needs the wrk2 binary (see tools/wrk2/) + cargo:
WRK2_BIN=tools/wrk2/wrk gates/wrk2_sweep.sh

# Gate 2 needs cargo + python3:
gates/gate2_oracle.sh

# Gate 3 needs cargo + python3 (coordinator + worker_node + mock binaries):
gates/gate3_merge.sh
```

Commit the refreshed `gateN.json` alongside the change that motivated the re-run,
so the gate status travels with the code that has to satisfy it.
