# Gate 3 — fleet merge correctness

Two workers each at half load must aggregate to the same result as one worker at full load. Proves HDR merge across the fleet is lossless.

## Setup
1. `mock/` healthy, fixed latency.
2. Run A: one worker at 2000 RPS, 60s. Record merged p50/p95/p99 + total count.
3. Run B: two workers at 1000 RPS each, same epoch (coordinator-synced), 60s.

## Pass criteria
- Run B total request count ≈ Run A (within 2%).
- Run B merged p50/p95/p99 within ±5% of Run A.
- Global offered rate in Run B is ~2000 (not two unsynced 1000s drifting) — verify via achieved_rps timeline.

## This gate exists because
The fleet only works if merge is lossless and schedule-sync holds. If half+half != full, either the HDR merge is wrong or the workers desynced (clock skew) — both are silent killers of the distributed design.
