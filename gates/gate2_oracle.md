# Gate 2 — oracle (the thesis is real)

The tool must report the failure rate we actually injected. This proves correctness measurement works, not just that it produces a number.

## Setup
1. `mock/` with a known load-dependent cliff: `GET /api?mode=fast500&cliff_rps=100&pct=100`.
   (Above 100 RPS, 100% of responses return a fast HTTP 500.)
2. Run engine + inline assertions (status tier) ramping through the cliff.

## Pass criteria
- Below 100 RPS: reported correctness ~100%.
- Above 100 RPS at full injection: reported fail_status rate matches injected % within ±2%.
- The correctness-vs-load curve shows the cliff at ~100 RPS.
- Latency stays roughly flat across the cliff (fast 500s are fast) — this is the headline: latency blind, correctness caught.

## Variants to also pass
- mode=truncate → fail_size and/or fail_schema fire (needs offload tier for schema).
- mode=wrong_value → fail_value fires (offload tier).
- mode=slow_ok → fail_latency fires, correctness stays high (slow but correct).

## This gate exists because
A tool that measures correctness must be validated against ground truth. We inject a known %, we must report that %. Without this, "we measure correctness" is unproven marketing.

## Runner
`gates/gate2_oracle.sh` automates the sweep across `fast500`, `slow_ok`, and
`truncate` modes. Each mode runs a 12-second engine session at 400 RPS against
a mock with `cliff_rps=100`, then asserts the reported tier-fail rate sits in
`[85 - 5, 100 + 5]%` (override with `ABOVE_FLOOR_PCT=` and `TOLERANCE_PCT=`).
The `wrong_value` mode is covered by `control/internal/offload/offload_test.go`
(7 unit tests against the path-tier evaluator) plus the integration test
`offload_sampling_captures_bodies_at_the_configured_cadence`. Add JSON-Schema
coverage when the oracle script grows a `--schema` variant.

Run: `gates/gate2_oracle.sh`. Exits 0 on agreement, 1 on any mode drift, 2 on
a setup error.
