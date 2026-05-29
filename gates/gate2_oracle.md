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
