-- FROZEN CONTRACT. Postgres + TimescaleDB. Do not edit without human sign-off.
--
-- HARD RULES:
--   1. API keys for benchmarked targets are NEVER stored. Anywhere.
--   2. assert_spec is JSONB and MUST contain no secrets.
--   3. target_headers values that look like auth get replaced with '***' before any persistence.
--   4. Tick-level writes from the coordinator must never block the request scheduler;
--      use a buffered writer in the control plane, not a synchronous insert per tick.

CREATE EXTENSION IF NOT EXISTS timescaledb;
CREATE EXTENSION IF NOT EXISTS pgcrypto;  -- gen_random_uuid()

-- ============================================================
-- RUNS (one row per benchmark execution)
-- ============================================================

CREATE TABLE runs (
  id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name                  TEXT,
  template_id           UUID,                          -- nullable; set if launched from a template
  -- target
  target_url            TEXT NOT NULL,
  target_method         TEXT NOT NULL CHECK (target_method IN ('GET','POST','PUT','PATCH','DELETE','HEAD')),
  target_headers_redacted JSONB NOT NULL,              -- auth values replaced with '***'
  target_body_size      INT,                           -- size only; body itself not stored unless explicit
  -- load shape
  target_rps            DOUBLE PRECISION NOT NULL,
  duration_s            INT NOT NULL,
  warmup_s              INT NOT NULL DEFAULT 0,
  load_model            TEXT NOT NULL CHECK (load_model IN ('open','closed')),
  connections           INT NOT NULL,
  keepalive             BOOLEAN NOT NULL,
  -- assertions
  assert_spec           JSONB NOT NULL,                -- schema/paths/patterns. NO secrets.
  rate_limit_policy     JSONB NOT NULL,
  -- outcomes (filled as run progresses)
  effective_rps         DOUBLE PRECISION,              -- achieved overall
  cliff_rps             DOUBLE PRECISION,              -- where correctness drops (NULL if no cliff)
  rate_limit_onset_rps  DOUBLE PRECISION,              -- where 429s began (NULL if never)
  saturation_rps        DOUBLE PRECISION,              -- where achieved stopped tracking offered
  -- final percentiles (corrected)
  p50_us  BIGINT, p95_us  BIGINT, p99_us  BIGINT, p999_us BIGINT,
  -- final percentiles (uncorrected — for the COO-error delta)
  u_p50_us BIGINT, u_p95_us BIGINT, u_p99_us BIGINT, u_p999_us BIGINT,
  -- final TTFB
  ttfb_p50_us BIGINT, ttfb_p99_us BIGINT,
  -- final histogram (full serialized HDR, V2 compressed) — for client-side render & merge
  final_corrected_hist   BYTEA,
  final_uncorrected_hist BYTEA,
  final_ttfb_hist        BYTEA,
  -- counts
  total_requests        BIGINT,
  total_pass            BIGINT,
  total_rate_limited    BIGINT,
  -- cost
  estimated_cost_usd    NUMERIC(10,4),
  cost_per_request_usd  NUMERIC(12,6),       -- user-declared; enables the ceiling
  actual_cost_usd       NUMERIC(10,4),
  -- lifecycle
  status                TEXT NOT NULL CHECK (status IN
                          ('draft','validated','queued','running','completed','failed','aborted')),
  status_reason         TEXT,
  warnings              JSONB,                          -- e.g. [{ "code":"WORKER_LOST", ... }]
  created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
  started_at            TIMESTAMPTZ,
  completed_at          TIMESTAMPTZ
);

CREATE INDEX idx_runs_status       ON runs (status) WHERE status IN ('queued','running');
CREATE INDEX idx_runs_template     ON runs (template_id) WHERE template_id IS NOT NULL;

-- ============================================================
-- RUN_TICKS (time-series; one row per second per run)
-- ============================================================

CREATE TABLE run_ticks (
  run_id          UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  ts              TIMESTAMPTZ NOT NULL,
  elapsed_s       INT NOT NULL,
  offered_rps     DOUBLE PRECISION,
  achieved_rps    DOUBLE PRECISION,
  -- running percentiles (run-to-date, corrected)
  p50_us  BIGINT, p95_us  BIGINT, p99_us  BIGINT, p999_us BIGINT,
  ttfb_p50_us BIGINT, ttfb_p99_us BIGINT,
  -- per-tick histogram snapshot (this-tick-only, for live charts; final lives on runs)
  this_tick_hist  BYTEA,
  -- bucketed correctness (jsonb array of { rps_lo, rps_hi, total, pass, fail_by_class })
  buckets         JSONB,
  -- aggregate counts (this tick only)
  total           BIGINT,
  pass            BIGINT,
  fail_status     BIGINT,
  fail_latency    BIGINT,
  fail_size       BIGINT,
  fail_schema     BIGINT,
  fail_value      BIGINT,
  fail_regex      BIGINT,
  fail_content_type BIGINT,
  fail_transport  BIGINT,
  rate_limited    BIGINT,
  timeouts        BIGINT,
  -- bytes + connections
  bytes_sent      BIGINT,
  bytes_recv      BIGINT,
  in_flight       INT,
  conn_errors     INT
);

SELECT create_hypertable('run_ticks', 'ts', if_not_exists => TRUE);
CREATE INDEX idx_ticks_run_ts ON run_ticks (run_id, ts);

-- correctness-vs-load (the headline query):
--   SELECT (buckets->>'rps_lo')::float AS rps,
--          SUM((buckets->>'pass')::bigint)::float / NULLIF(SUM((buckets->>'total')::bigint), 0) AS pass_rate
--     FROM run_ticks, jsonb_array_elements(buckets) AS buckets
--    WHERE run_id = $1
--    GROUP BY rps ORDER BY rps;

-- ============================================================
-- WORKER_TELEMETRY (per-worker per-tick — for diagnosing fleet behavior)
-- ============================================================

CREATE TABLE worker_telemetry (
  run_id        UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  worker_id     UUID NOT NULL,
  ts            TIMESTAMPTZ NOT NULL,
  sched_slip_us BIGINT,
  client_saturated BOOLEAN,
  cpu_pct       REAL,
  rss_bytes     BIGINT,
  fd_count      INT,
  status        TEXT      -- 'running','lost','respawned'
);

SELECT create_hypertable('worker_telemetry', 'ts', if_not_exists => TRUE);
CREATE INDEX idx_telemetry_run_worker ON worker_telemetry (run_id, worker_id, ts);

-- ============================================================
-- OFFLOAD_EVAL (results of expensive-tier assertion checks on sampled bodies)
-- ============================================================

CREATE TABLE offload_eval (
  id              BIGSERIAL PRIMARY KEY,
  run_id          UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  ts              TIMESTAMPTZ NOT NULL,
  rps_at_send     DOUBLE PRECISION NOT NULL,    -- bucketing key
  status          INT NOT NULL,
  corrected_lat_us BIGINT,
  -- verdict
  verdict         TEXT NOT NULL CHECK (verdict IN
                    ('pass','fail_schema','fail_value','fail_regex')),
  details         JSONB,                          -- which path/pattern failed, error msg
  -- sampled body: stored ONLY if assert_spec.store_sampled_bodies = true
  body_size       INT,
  body_sha256     BYTEA,                          -- always, for dedup/proof
  body            BYTEA                           -- nullable; truncated to assert_spec cap
);

CREATE INDEX idx_offload_run_rps ON offload_eval (run_id, rps_at_send);
CREATE INDEX idx_offload_verdict ON offload_eval (run_id, verdict);

-- ============================================================
-- TEMPLATES (re-runnable configs; secrets always redacted)
-- ============================================================

CREATE TABLE templates (
  id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name              TEXT NOT NULL,
  -- same shape as a run spec, but auth values in target_headers are '***'
  spec_redacted     JSONB NOT NULL,
  created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_used_at      TIMESTAMPTZ
);

CREATE INDEX idx_templates_created ON templates (created_at DESC);

-- ============================================================
-- COMPARISONS (group N runs as a saved comparison set)
-- ============================================================

CREATE TABLE comparisons (
  id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name        TEXT,
  run_ids     UUID[] NOT NULL,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ============================================================
-- DATA RETENTION
-- ============================================================
-- Tick tables grow fast. Suggested policy (not enforced in schema; ops choice):
--   - run_ticks: keep raw 30d, downsample to 10s after that
--   - worker_telemetry: keep 7d raw, drop after
--   - offload_eval bodies: keep 7d, then NULL out body but keep verdict + sha256
-- Use Timescale retention/compression policies.

-- ============================================================
-- SECRET-SCAN GUARDRAIL (verified by control-plane test, not enforceable in SQL)
-- ============================================================
-- A passing test runs: known canary key passed in target.headers → grep all
-- tables, logs, dumps → must find zero occurrences. The schema cannot enforce
-- this; the control plane and its scrubbing pipeline must.
