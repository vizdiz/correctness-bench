# Correctness Under Load Gate

A GitHub Action that benchmarks an API under load and **fails the build when
correctness or p99 latency regresses** past a threshold versus a baseline.

A fast HTTP 500, a truncated body, or a wrong value is a failure here, not a
pass. Correctness is measured as a function of load, so the gate catches the
degradation a latency check or a smoke test misses: the service stays fast while
it starts returning wrong answers past some RPS.

## Usage

```yaml
- uses: vizdiz/correctness-bench/.github/actions/correctness-gate@main
  with:
    control-url: ${{ secrets.BENCH_CONTROL_URL }}
    target-url: https://staging.example.com/api/checkout
    rps: '1000'
    duration: '60'
    expected-status: '200'
    max-latency-ms: '200'
    api-key: ${{ secrets.TARGET_API_KEY }}          # optional; Bearer to the target, never persisted
    baseline-run-id: ${{ vars.BENCH_BASELINE_RUN_ID }}
    p99-delta-pct: '10'
    correctness-delta-pct: '1'
```

See [`correctness-gate.example.yml`](../../workflows/correctness-gate.example.yml)
for a full PR workflow.

## How it works

1. Downloads the `bench` CLI (a thin client of your control plane).
2. Runs the benchmark: `bench run ...`, capturing the new run id.
3. If `baseline-run-id` is set, runs `bench regression <run> --baseline <baseline>`
   and **fails the job (exit 1)** when the p99 or correctness delta exceeds the
   thresholds. With no baseline it is record-only (benchmark, no gate).
4. Writes a summary (run link, verdict) to the job summary.

## Inputs

| Input | Required | Default | Notes |
|-------|----------|---------|-------|
| `control-url` | yes | | Base URL of your correctness-bench control plane. |
| `target-url` | yes | | The API to benchmark. |
| `method` | no | `GET` | |
| `rps` | no | `100` | Target requests/sec. |
| `duration` | no | `30` | Seconds. |
| `connections` | no | `50` | Pool size. |
| `expected-status` | no | `200` | Space-separated (empty = any 2xx). |
| `max-latency-ms` | no | | A slower response counts as a correctness failure. |
| `name` | no | `ci-<sha>` | Run name in the dashboard. |
| `api-key` | no | | Bearer token for the target. Pass a secret; never persisted. |
| `baseline-run-id` | no | | Finalized baseline run id. Omit to skip the gate. |
| `p99-delta-pct` | no | `10` | Max p99 regression, percent. |
| `correctness-delta-pct` | no | `1` | Max correctness drop, absolute points. |
| `web-url` | no | = control-url | For the dashboard link. |
| `bench-version` | no | `latest` | Release tag of the CLI. |

## Outputs

| Output | Description |
|--------|-------------|
| `run-id` | The run this gate created. |
| `dashboard-url` | Deep link to the run. |
| `passed` | `"true"` if completed and the gate passed. |

## Establishing a baseline

Run the same benchmark against your main branch, take the printed run id, and
store it as the `BENCH_BASELINE_RUN_ID` repo variable. Re-record it whenever you
intend to move the baseline.

## Prerequisites

- A reachable control plane (self-hosted `docker-compose`, or the managed tier).
- A published `bench` CLI release asset named `bench-x86_64-unknown-linux-gnu`
  (Linux x86_64). Publish them by tagging a version -
  `git tag v0.1.0 && git push --tags` - which runs `.github/workflows/release.yml`
  to cross-build `bench` and attach the binaries to the release.
