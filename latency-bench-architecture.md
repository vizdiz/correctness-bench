# API Correctness-Under-Load Benchmarker

**Thesis.** Correctness as a continuous function of load. wrk2 misses it (counts fast 500s as wins). Postman misses it (one request). Headline: latency flat, correctness cliffs.

**Stack.** Rust workers + coordinator (Tokio, hot path) · Go control plane · Postgres+Timescale · web/CLI/MCP.

**Hard rules.** Workers never persist. Control plane never generates load. Slow DB writes never back-pressure the scheduler. Target API keys: in-memory only, never persisted, scrubbed from logs/exports.

**HDR histogram.** Bounded error, merges losslessly. Worker keeps its own; coordinator adds.

---

## 1. Components

![architecture](img/architecture.svg)

---

## 2. Worker (Rust)

Stateless. Owns a load slice.

**COO-correct scheduling.** Send times fixed against a timeline. Request `n` sends at `start + n/throughput`. Behind → catch up at 2× (matched to wrk2 for gate #1).

**Latency from intended send, not actual.** Stalls accrue as latency to requests that should've fired. Two HDRs: corrected + uncorrected. Delta = COO error, kept as proof.

**Inline assertions only.** Near-zero tier (transport/status/latency/size/content-type). Bucketed by offered-RPS at send time. Expensive checks: sample, ship up, off hot path.

**Self-instrumentation.** Tracks scheduling slip. Emits `client_saturated`. Never silently report client bottleneck as server latency.

**Tick.** 1s: serialize partial HDRs + bucketed counts → coordinator over gRPC.

**Tradeoff.** Tokio < wrk2's C single-box. We scale horizontally; throughput-per-box isn't the bottleneck. wrk2 is the gate-#1 oracle, not the engine.

---

## 3. Assertion layer (the thesis)

![assertion tiers](img/assertion-tiers.svg)

**Offload.** Worker samples every Nth response, ships body to control-plane eval pool. Coverage caps RPS — explicit knob, surfaced in UI.

**Failure classes, distinct.** Transport vs status vs correctness vs latency. 429 separated as our artifact, never folded into the API's score.

---

## 4. Coordinator (Rust)

Per-run, in-memory. SPOF for a *run*, never for data.

**Split + sync.** Workers schedule against a shared coordinator epoch (not local wall clock — skew desyncs the global rate).

**Comparison = interleave.** N targets round-robined under ONE schedule. Sequential confounds.

**Merge.** Partial HDRs deserialized + summed losslessly each tick.

**Worker death.** Redistribute slice to survivors with headroom; else respawn (cold start). Record the gap as lost load — silent recovery would under-report the tail (COO on ourselves). "Stateless" = no durable state; ephemeral schedule position + partial HDR are recoverable. 1s tick is the durability knob.

---

## 5. Control plane (Go)

Lifecycle, REST + SSE, target-key custody, offload eval pool. Iterate-fast tier; Rust stays on the hot path.

**Lifecycle.** `draft → validated → queued → running → (completed | failed | aborted)`. Abort propagates <1s (metered-API money/ban risk).

**API.** REST + SSE: create / status / live stream / abort / compare / regression-check / templates. SSE over WS (one-way is enough).

**Target keys.** TLS transit, memory for the run, never persisted (anywhere), scrubbed from logs/traces/dumps, stripped from exports/shares, header-flexible (Bearer/x-api-key/query param). Re-runnable configs store redacted placeholders.

**No user auth in v1.** Single-user deployment. Deploy behind a network boundary. Auth comes back if/when multi-tenant.

---

## 6. Data model

Postgres + Timescale. Target keys NEVER stored.

`runs`: spec, redacted headers, outcomes (effective_rps, cliff_rps, rate_limit_onset_rps, percentiles corrected + uncorrected, TTFB, costs, warnings).

`run_ticks` (hypertable): offered/achieved RPS, percentiles-to-date, bucketed counts by failure class, 429s, bytes.

`worker_telemetry` (hypertable): per-worker slip, saturation, status (running/lost/respawned).

`offload_eval`: sampled bodies (optional storage) + verdicts (pass / fail_schema / fail_value / fail_regex), keyed by rps_at_send.

`templates`: re-runnable specs, secrets redacted to `***`.

Correctness-vs-load query: `SUM(pass)/SUM(total) GROUP BY rps_bucket`. The cliff is in the data.

---

## 7. Mock target + bench (Rust)

Deployed service we control. Triple duty: validation bench vs wrk2; cliff demo; ground-truth oracle.

Failure injection load-dependent. Modes: `healthy`, `fast500` (latency-blind), `truncate` (size+schema), `wrong_value` (value tier), `slow_ok` (latency tier).

**Gate #1 (agreement).** Corrected p50/p95/p99 must match wrk2 ±5% on healthy. Disagree → engine bug. Nothing matters until green.

**Gate #2 (oracle).** Inject known fail %, reported % must match ±2%. Proves the measurement.

**Gate #3 (merge).** Two half-load workers ≈ one full-load run. Proves HDR merge + schedule sync.

---

## 8. Metrics + viz

All metrics bucketed by offered-RPS. They're functions of load, not scalars.

**Latency.** p50/p95/p99/p999, corrected + uncorrected (COO delta visible), TTFB split.

**Correctness.** Pass rate + fail-by-class (transport, status, size, schema, value, latency). Rate-limited (429) tracked separately as our artifact; first-429-RPS surfaced as a measured property.

**Load/health.** Offered vs achieved, effective RPS after worker loss, saturation point, per-worker scheduling slip.

![the cliff](img/cliff.png)

The whole pitch: blue p99 looks healthy, green correctness cliffs. wrk2 shows only blue. Postman shows one green dot.

**Four viz.** Headline (above). Latency histogram, log-scale x, corrected + ghosted uncorrected. Time-series over duration. Comparison overlay.

---

## 9. Interfaces

**Web (TS).** Headline chart + histogram + time-series + comparison. SSE-live. Must not look AI-generated — real design system.

**CLI (Rust).** Thin client of the control plane. Starts a run, streams progress, prints a dashboard link. Exit codes for run success/failure, no threshold gating in v1 (threshold/query logic is a separate future project).

**MCP (TS).** Engine as agent tools: run, get-results, compare, regression-check. The thing neither wrk2 nor Postman offers.

---

## 10. Build order

1. Mock healthy mode.
2. Single Rust worker, COO-correct, HDR. **Gate #1.** Nothing matters until green.
3. Control plane + web UI, live data flowing.
4. Inline assertions + correctness-vs-load chart, inject fast-500. **Gate #2.**
5. Coordinator + 2nd worker. **Gate #3.**
6. Comparison + decision layer (cost, error rate, tail-vs-load).
7. Offload eval (expensive tier) + coverage knob.
8. Target-key hardening + cost/abort guards.
9. CLI → MCP.

---

## 11. Tech choices

![tradeoffs](img/tradeoffs.svg)

**Open.** Worker↔coord transport drafted gRPC, revisit if framed-TCP suffices. Coordinator in Rust (shares HDR types, on timing path), could fold into Go to drop a language.

---

## 12. Positioning + non-goals

**Not monitoring.** Datadog/CloudWatch passively watch your production; this actively probes systems you may not own, pre-adoption, outside-in.

**Not just k6 with checks.** Correctness as a continuous measured axis (the cliff), not a pass/fail gate. Decision triad in one view: latency + correctness + cost.

**Permanent non-goals.** Continuous monitoring / alerting / dashboards. Geographic multi-region probes. Target-side resource metrics. Payload-size sweeps. Recurring runs allowed for vendor-regression decision-support only, never as an alerting layer. No user auth in v1 (single-user / network-boundary deployment).

---

## 13. Phasing (what ships when)

**Phase 1 — demoable v1 (~1.5 weeks).** Must pass gates #1, #2, #3. The minimum that proves the thesis:
- Rust engine: single worker → coordinator + dynamic fleet (4 workers default).
- Go control plane: lifecycle, REST + SSE, target-key custody with canary test, offload eval pool.
- Web: single-page UI, the four viz, live SSE.
- Mock target deployed alongside on the same VM.
- CLI: runs a bench, prints dashboard link.
- Logging: structured JSON to stdout, viewed via `docker-compose logs`.
- Cost handling: user-declared `cost_per_request_usd` + auto-abort on budget overrun.
- Property-based tests on the COO scheduler. Differential matrix vs wrk2 across (RPS × open/closed × keepalive) configurations.

**Phase 2 — added after Phase 1 ships (or sooner if Phase 1 finishes early).**
- MCP server.
- OpenTelemetry tracing across the three languages.
- Centralized log aggregation (Loki or similar).
- Screenshot regression tests on the web (Playwright).
- Web UI evolves toward multi-page product (run history, navigation).
- Query language as its own project, consuming the API.

**Deferred / explicitly out:** continuous monitoring; geo probes; resource-side metrics; payload sweeps; user auth; pricing-table for popular APIs.

---

## 14. Deployment + ops

**Topology.** Single cloud VM running the same `docker-compose.yml` as local dev. Services: `control`, `coordinator`, `worker-1..N`, `mock`, `postgres` (with Timescale). All on one Docker network.

**Worker discovery (dynamic).** Workers POST `/workers/register` to the coordinator on startup with their hostname:port. Coordinator keeps an in-memory `{worker_id → address}` map. Heartbeats handle liveness; if a worker drops, the existing redistribute-or-respawn logic in §4 kicks in. No Consul / etcd / Kubernetes for v1; Docker DNS resolves service names within the network.

**TLS to targets.** Workers verify TLS by default. `verify_tls = false` flag for self-signed / dev. No client certs, no proxy support in v1.

**Logging.** JSON to stdout per service. `docker-compose logs <service>` is the v1 debugging surface.

**Demo.** Five-minute script: (1) `docker-compose up` shows the fleet starting and registering, (2) trigger a run against the mock in healthy mode → headline chart updates live via SSE, (3) switch the mock to fast-500 cliff mode → re-run, watch latency stay flat while correctness collapses past the cliff RPS, (4) kill a worker mid-run → coordinator redistributes, effective RPS shown reduced, (5) optional: comparison view of mock-healthy vs mock-degrading side by side.

