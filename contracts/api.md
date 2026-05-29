# API contract — control plane (FROZEN)

REST + SSE. Client (web/CLI/MCP) ↔ control plane. Worker↔coordinator is `bench.proto`. Do not edit without human sign-off.

Versioned: `v1`. Path prefix `/v1`. Contract bumps go on a new prefix; the old one stays alive through deprecation.

## Conventions
- All timestamps ISO-8601 UTC.
- All durations integers in microseconds (latency) or seconds (run duration), suffix in the field name (`_us`, `_s`).
- All IDs are UUIDs as strings.
- All responses JSON unless noted.
- Numbers never quoted as strings.

## Auth
**No user auth in v1.** Single-user deployment, behind a network boundary (local, VPN, or tunnel). Multi-tenancy is a future concern; if it returns, it returns on `/v2`.

**Target API keys** (the keys we use to call the API being benchmarked) are the only credentials in play:
  - Sent in `target.headers` on `POST /v1/runs`.
  - Held in memory for the run duration only.
  - **NEVER persisted, NEVER returned in any response, NEVER logged.**
  - Stripped from any export, share link, or saved config.
  - If saved as a re-runnable template (see `/v1/templates`), the value is replaced with a redacted placeholder `"***"`; re-supply on re-run.

## Error model
All errors return JSON with HTTP status:
```json
{
  "error": {
    "code": "INVALID_RUN_SPEC",
    "message": "target_rps must be > 0",
    "field": "target_rps",
    "request_id": "uuid"
  }
}
```
Error codes:
- `400 INVALID_RUN_SPEC` — validation failed (`field` populated)
- `404 NOT_FOUND`
- `409 CONFLICT` — e.g. abort on already-completed run
- `429 RATE_LIMITED` — control plane self-protection (separate from target's 429s)
- `500 INTERNAL`
- `503 NO_CAPACITY` — no workers available; try again later

## Endpoints

---

### POST /v1/runs
Create + queue a run.

Request:
```json
{
  "name": "openai vs anthropic — 100rps",
  "target": {
    "url": "https://api.example.com/v1/foo",
    "method": "POST",
    "headers": { "Authorization": "Bearer sk-...", "Content-Type": "application/json" },
    "body_base64": "...",
    "timeout_ms": 30000,
    "verify_tls": true
  },
  "target_rps": 100,
  "duration_s": 60,
  "warmup_s": 5,
  "load_model": "open",
  "connections": 50,
  "keepalive": true,
  "assert": {
    "max_latency_us": 200000,
    "min_body_bytes": 2,
    "max_body_bytes": 1048576,
    "content_type": "application/json",
    "expected_status": [200],
    "schema": { "$schema": "...", "type": "object", "required": ["status"] },
    "paths": [ { "path": "$.status", "equals": "ok" } ],
    "patterns": ["^\\{"],
    "sample_every_n": 50
  },
  "rate_limit_policy": {
    "action": "backoff",
    "max_backoff_ms": 5000,
    "record_onset": true
  },
  "estimated_cost_usd": 0.42,
  "cost_per_request_usd": 0.001
}
```
Response `201 Created`:
```json
{ "run_id": "uuid", "status": "queued", "estimated_cost_usd": 0.42 }
```
Notes:
- `target.body_base64` carries raw request body; the control plane decodes for the worker.
- `target.headers` is write-only. Stripped before persistence; never echoed in any response.
- **Cost ceiling.** If `cost_per_request_usd` is set: control plane refuses to start runs where `target_rps × duration_s × cost_per_request_usd > estimated_cost_usd × 1.1`, and auto-aborts in-flight runs whose cumulative requests would exceed that ceiling. If `cost_per_request_usd` is unset, cost gating is off. The tool is not a pricing oracle; the user declares per-request cost.

---

### GET /v1/runs/:id
Status + final results. **`target.headers` MUST NOT be present in the response.**

Response (running):
```json
{
  "run_id": "uuid",
  "name": "openai vs anthropic — 100rps",
  "status": "running",
  "started_at": "2026-05-28T18:00:00Z",
  "target": { "url": "...", "method": "POST" },
  "target_rps": 100,
  "effective_rps": 98.2,
  "elapsed_s": 23
}
```
Response (completed):
```json
{
  "run_id": "uuid",
  "status": "completed",
  "started_at": "...", "completed_at": "...",
  "target_rps": 100, "effective_rps": 98.2,
  "percentiles": {
    "corrected":   { "p50_us": 0, "p95_us": 0, "p99_us": 0, "p999_us": 0 },
    "uncorrected": { "p50_us": 0, "p95_us": 0, "p99_us": 0, "p999_us": 0 },
    "ttfb":        { "p50_us": 0, "p99_us": 0 }
  },
  "correctness_curve": [
    { "rps_lo": 0,   "rps_hi": 10,  "total": 600, "pass": 600, "fail_by_class": {} },
    { "rps_lo": 90,  "rps_hi": 100, "total": 5800, "pass": 3500,
      "fail_by_class": { "fail_status": 2300 } }
  ],
  "cliff_rps": 95.0,
  "rate_limit": {
    "onset_rps": 150.0,
    "total_count": 4200,
    "backoff_us_total": 12000000
  },
  "histogram_url": "/v1/runs/{id}/histogram",
  "warnings": [
    { "code": "WORKER_LOST", "message": "ran 8200 of 10000 RPS (1 of 5 workers lost)" }
  ]
}
```

---

### GET /v1/runs/:id/histogram
Returns the full serialized HDR (compressed binary) for client-side rendering of the latency distribution. `Content-Type: application/hdr-v2+gzip`.

---

### GET /v1/runs/:id/stream  (SSE)
Live, server→client. `Content-Type: text/event-stream`.

Event types (each `data:` payload is JSON):

`tick` — once per second per run:
```json
{
  "type": "tick",
  "ts": "2026-05-28T18:00:23Z",
  "elapsed_s": 23,
  "offered_rps": 100,
  "achieved_rps": 98.2,
  "percentiles_so_far": { "p50_us": 0, "p99_us": 0 },
  "this_tick": { "total": 98, "pass": 90, "fail_by_class": { "fail_status": 8 } },
  "buckets": [ { "rps_lo": 90, "rps_hi": 100, "pass_rate": 0.91 } ],
  "rate_limited_this_tick": 0
}
```

`status` — `{ "type": "status", "status": "running" | "completed" | "failed" | "aborted" }` (status is one of those four)

`warning` — non-fatal issues: `{ "type": "warning", "code": "CLIENT_SATURATED", "message": "..." }`

`done` — final results object (same shape as `GET /v1/runs/:id` completed).

Client SHOULD reconnect with `Last-Event-ID` on disconnect; server resumes from that tick.

---

### POST /v1/runs/:id/abort
Hard stop. Must propagate to workers in <1s.

Response: `{ "status": "aborted", "aborted_at": "..." }`.
`409 CONFLICT` if the run is already in a terminal state.

---

### GET /v1/runs?status=&limit=&cursor=
Paginated list of runs. Cursor-based.

Response:
```json
{
  "runs": [ { "_": "compact run summary objects" } ],
  "next_cursor": "opaque-string-or-null"
}
```

---

### GET /v1/runs/:id/compare/:id2
Side-by-side. Returns both result objects plus:
```json
{
  "a": { "_": "completed run object — see GET /v1/runs/:id" },
  "b": { "_": "completed run object — see GET /v1/runs/:id" },
  "winner_by": {
    "latency_p99":      "a",
    "correctness":      "b",
    "cost_per_request": "a",
    "tail_stability":   "b",
    "rate_limit_onset": "b"
  },
  "fairness": {
    "interleaved": true,
    "same_assert_spec": true,
    "same_load_shape": true
  }
}
```
`fairness` flags expose whether the comparison was apples-to-apples; the UI surfaces a banner if any is false.

---

### POST /v1/runs/:id/regression-check
Compare a freshly-completed run against a stored baseline.

Request: `{ "baseline_run_id": "uuid", "thresholds": { "p99_delta_pct": 10, "correctness_delta_pct": 1 } }`

Response:
```json
{
  "passed": false,
  "p99_delta_pct": 18.4,
  "correctness_delta_pct": -3.1,
  "cliff_rps_delta": -22.0,
  "details": "p99 regressed by 18.4% (threshold 10%)"
}
```

---

### Templates (saved re-runnable configs)
Target API keys are NEVER stored — saved as redacted placeholders.

- `POST /v1/templates` — body is a run spec; `target.headers` values containing auth-like strings are replaced with `"***"` before storage.
- `GET /v1/templates`
- `POST /v1/templates/:id/run` — body re-supplies the redacted secrets.

---

## SSE reconnection semantics
- Each event carries `id: <tick_number>`.
- On reconnect with `Last-Event-ID: N`, server replays from tick N+1 (events buffered for at least 30s).
- If buffer is gone, server sends a single `resume_gap` event and continues from "now."

## Status enum
`draft | validated | queued | running | completed | failed | aborted`

Transitions:
```
draft → validated → queued → running → (completed | failed | aborted)
```
Terminal states are immutable.
