//! Mock target with load-dependent failure injection.
//!
//! A real deployable HTTP service whose failure behavior is a function of the
//! CURRENT offered RPS, so a *cliff* appears at a threshold rather than as
//! uniform noise. Everything in the project validates against this.
//!
//! Endpoint: `GET|POST /api` with query knobs:
//!   - `mode`            one of healthy | fast500 | truncate | wrong_value | slow_ok | ratelimit
//!   - `cliff_rps`       injection turns on once measured RPS exceeds this
//!   - `pct`             percentage of above-cliff requests that get the failure
//!   - `base_latency_ms` fixed latency applied to every response
//!
//! Also `GET /healthz` (cheap, not counted toward the RPS meter).
//!
//! Determinism: above-cliff injection selects requests by a per-instance request
//! counter (`n % 100 < pct`), so demos and gate runs reproduce exactly.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde::Deserialize;

/// Extra latency (on top of base) applied to `slow_ok` above the cliff. Chosen
/// well above the api.md example `max_latency_us` (200 ms) so the latency tier
/// reliably fires while correctness stays green.
pub const SLOW_PENALTY_MS: u64 = 500;

/// Canonical correct response body (schema-valid, correct values).
pub const HEALTHY_BODY: &str = r#"{"status":"ok","count":3,"items":["a","b","c"]}"#;
/// Schema-valid JSON with a wrong value (count must be >= 0; this is -1).
pub const WRONG_VALUE_BODY: &str = r#"{"status":"ok","count":-1,"items":["a","b","c"]}"#;
/// Truncated / invalid JSON: claims application/json but the body does not parse
/// and is short (trips both the size tier and the schema tier).
pub const TRUNCATE_BODY: &str = r#"{"status":"ok","count":3,"items":["a","b"#;
/// Fast error body — a 5xx that returns at normal latency (latency-blind).
pub const ERROR_BODY: &str = r#"{"status":"error","code":500}"#;
/// Fast rate-limit body — a 429 that returns at normal latency (latency-blind).
pub const RATELIMIT_BODY: &str = r#"{"status":"error","code":429}"#;

// ============================================================
// Rolling 1-second RPS meter
// ============================================================

const SLOT_MS: u64 = 100;
const SLOTS: u64 = 10; // 10 * 100ms = 1000ms window

struct Bucket {
    slot: AtomicU64,
    count: AtomicU64,
}

/// Lock-free-ish rolling-1s request meter. Each 100ms slot owns a bucket; the
/// sum of buckets whose slot id falls inside the last second is the current RPS.
/// Slight boundary inaccuracy is fine — it only gates injection behavior.
pub struct RpsMeter {
    buckets: Vec<Bucket>,
}

impl RpsMeter {
    fn new() -> Self {
        let buckets = (0..SLOTS)
            .map(|_| Bucket {
                slot: AtomicU64::new(u64::MAX),
                count: AtomicU64::new(0),
            })
            .collect();
        Self { buckets }
    }

    /// Record one request observed at `now_ms` (ms since meter start).
    pub fn record(&self, now_ms: u64) {
        let slot = now_ms / SLOT_MS;
        let b = &self.buckets[(slot % SLOTS) as usize];
        loop {
            let cur = b.slot.load(Ordering::Acquire);
            if cur == slot {
                b.count.fetch_add(1, Ordering::Relaxed);
                return;
            }
            // Stale (or never-used) bucket: claim it for this slot, reset to 1.
            if b
                .slot
                .compare_exchange(cur, slot, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                b.count.store(1, Ordering::Relaxed);
                return;
            }
            // Lost the race; retry.
        }
    }

    /// Current requests-in-the-last-second (≈ RPS) as of `now_ms`.
    pub fn current_rps(&self, now_ms: u64) -> f64 {
        let slot = now_ms / SLOT_MS;
        let oldest = slot.saturating_sub(SLOTS - 1);
        let mut sum = 0u64;
        for b in &self.buckets {
            let s = b.slot.load(Ordering::Acquire);
            if s != u64::MAX && s >= oldest && s <= slot {
                sum += b.count.load(Ordering::Relaxed);
            }
        }
        sum as f64
    }
}

// ============================================================
// Modes + query params
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Healthy,
    Fast500,
    Truncate,
    WrongValue,
    SlowOk,
    Ratelimit,
}

#[derive(Debug, Deserialize)]
pub struct Params {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default = "default_cliff")]
    pub cliff_rps: f64,
    #[serde(default = "default_pct")]
    pub pct: f64,
    #[serde(default)]
    pub base_latency_ms: u64,
}

fn default_cliff() -> f64 {
    100.0
}
fn default_pct() -> f64 {
    100.0
}

/// What the handler decided to do, computed from knobs + current load. Pure and
/// unit-testable; the handler just executes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub above_cliff: bool,
    pub injected: bool,
    pub status: StatusCode,
    pub body: &'static str,
    /// Total latency to apply (base, plus slow penalty for slow_ok injections).
    pub latency_ms: u64,
    /// `Retry-After` seconds to attach, if any (ratelimit injections only).
    pub retry_after_secs: Option<u64>,
}

/// Decide the response purely from knobs, measured RPS, and the request's
/// sequence number. No I/O.
pub fn decide(p: &Params, rps: f64, seq: u64) -> Decision {
    let above_cliff = rps > p.cliff_rps;
    // Deterministic selection of which above-cliff requests get the failure.
    let selected = (seq % 100) < p.pct.clamp(0.0, 100.0) as u64;
    let injected = above_cliff && selected && p.mode != Mode::Healthy;

    if !injected {
        return Decision {
            above_cliff,
            injected: false,
            status: StatusCode::OK,
            body: HEALTHY_BODY,
            latency_ms: p.base_latency_ms,
            retry_after_secs: None,
        };
    }

    match p.mode {
        Mode::Healthy => unreachable!("healthy never injects"),
        Mode::Fast500 => Decision {
            above_cliff,
            injected: true,
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: ERROR_BODY,
            latency_ms: p.base_latency_ms, // latency-blind: same latency as healthy
            retry_after_secs: None,
        },
        Mode::Truncate => Decision {
            above_cliff,
            injected: true,
            status: StatusCode::OK,
            body: TRUNCATE_BODY,
            latency_ms: p.base_latency_ms,
            retry_after_secs: None,
        },
        Mode::WrongValue => Decision {
            above_cliff,
            injected: true,
            status: StatusCode::OK,
            body: WRONG_VALUE_BODY,
            latency_ms: p.base_latency_ms,
            retry_after_secs: None,
        },
        Mode::SlowOk => Decision {
            above_cliff,
            injected: true,
            status: StatusCode::OK,
            body: HEALTHY_BODY, // correct body, just late
            latency_ms: p.base_latency_ms + SLOW_PENALTY_MS,
            retry_after_secs: None,
        },
        Mode::Ratelimit => Decision {
            above_cliff,
            injected: true,
            status: StatusCode::TOO_MANY_REQUESTS,
            body: RATELIMIT_BODY,
            latency_ms: p.base_latency_ms, // latency-blind: same latency as healthy
            retry_after_secs: Some(1),
        },
    }
}

// ============================================================
// App state + handlers
// ============================================================

pub struct AppState {
    start: Instant,
    meter: RpsMeter,
    counter: AtomicU64,
}

impl AppState {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            meter: RpsMeter::new(),
            counter: AtomicU64::new(0),
        }
    }
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

/// Build the router with a fresh, isolated state. Tests spawn this on an
/// ephemeral port; each call is independent (counter + meter are per-instance).
pub fn app() -> Router {
    let state = Arc::new(AppState::new());
    Router::new()
        .route("/api", get(handle).post(handle))
        .route("/healthz", get(healthz))
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn handle(State(st): State<Arc<AppState>>, Query(params): Query<Params>) -> Response {
    let now = st.now_ms();
    st.meter.record(now);
    let rps = st.meter.current_rps(now);
    let seq = st.counter.fetch_add(1, Ordering::Relaxed);

    let d = decide(&params, rps, seq);

    if d.latency_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(d.latency_ms)).await;
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert("x-mock-mode", mode_header(params.mode));
    headers.insert("x-mock-rps", HeaderValue::from(rps as u64));
    headers.insert("x-mock-above-cliff", bool_header(d.above_cliff));
    headers.insert("x-mock-injected", bool_header(d.injected));
    if let Some(secs) = d.retry_after_secs {
        headers.insert(header::RETRY_AFTER, HeaderValue::from(secs));
    }

    (d.status, headers, d.body).into_response()
}

fn bool_header(b: bool) -> HeaderValue {
    HeaderValue::from_static(if b { "true" } else { "false" })
}

fn mode_header(m: Mode) -> HeaderValue {
    HeaderValue::from_static(match m {
        Mode::Healthy => "healthy",
        Mode::Fast500 => "fast500",
        Mode::Truncate => "truncate",
        Mode::WrongValue => "wrong_value",
        Mode::SlowOk => "slow_ok",
        Mode::Ratelimit => "ratelimit",
    })
}

// ============================================================
// Unit tests (meter + decision logic — pure, no network)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn params(mode: Mode, cliff: f64, pct: f64) -> Params {
        Params {
            mode,
            cliff_rps: cliff,
            pct,
            base_latency_ms: 0,
        }
    }

    #[test]
    fn meter_counts_within_one_second_window() {
        let m = RpsMeter::new();
        for i in 0..50 {
            m.record(i); // 50 requests across the first 50ms
        }
        // All within the first second.
        assert_eq!(m.current_rps(60), 50.0);
    }

    #[test]
    fn meter_drops_old_slots() {
        let m = RpsMeter::new();
        for i in 0..10 {
            m.record(i * 100); // one request per 100ms slot, slots 0..9
        }
        assert_eq!(m.current_rps(900), 10.0);
        // Advance ~1.5s: slots 0..4 are now older than the 1s window.
        // At t=1500ms (slot 15), window covers slots 6..15 -> only slots 6,7,8,9 survive.
        assert_eq!(m.current_rps(1500), 4.0);
    }

    #[test]
    fn below_cliff_always_healthy_even_in_injection_mode() {
        // rps below cliff -> healthy regardless of mode.
        for mode in [
            Mode::Fast500,
            Mode::Truncate,
            Mode::WrongValue,
            Mode::SlowOk,
            Mode::Ratelimit,
        ] {
            let d = decide(&params(mode, 100.0, 100.0), 50.0, 0);
            assert!(!d.injected);
            assert_eq!(d.status, StatusCode::OK);
            assert_eq!(d.body, HEALTHY_BODY);
        }
    }

    #[test]
    fn above_cliff_injects_per_mode() {
        let rps = 200.0;
        assert_eq!(
            decide(&params(Mode::Fast500, 100.0, 100.0), rps, 0).status,
            StatusCode::INTERNAL_SERVER_ERROR
        );
        let trunc = decide(&params(Mode::Truncate, 100.0, 100.0), rps, 0);
        assert_eq!(trunc.status, StatusCode::OK);
        assert_eq!(trunc.body, TRUNCATE_BODY);
        let wrong = decide(&params(Mode::WrongValue, 100.0, 100.0), rps, 0);
        assert_eq!(wrong.body, WRONG_VALUE_BODY);
        let slow = decide(&params(Mode::SlowOk, 100.0, 100.0), rps, 0);
        assert_eq!(slow.status, StatusCode::OK);
        assert_eq!(slow.body, HEALTHY_BODY);
        assert_eq!(slow.latency_ms, SLOW_PENALTY_MS);
        let rl = decide(&params(Mode::Ratelimit, 100.0, 100.0), rps, 0);
        assert_eq!(rl.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(rl.retry_after_secs, Some(1));
    }

    #[test]
    fn healthy_mode_never_injects_above_cliff() {
        let d = decide(&params(Mode::Healthy, 0.0, 100.0), 9999.0, 0);
        assert!(!d.injected);
        assert_eq!(d.status, StatusCode::OK);
    }

    #[test]
    fn pct_selects_a_fraction_above_cliff() {
        // pct=50 over a contiguous block of 100 sequence numbers -> exactly 50 injected.
        let p = params(Mode::Fast500, 0.0, 50.0);
        let injected = (0..100).filter(|&n| decide(&p, 10.0, n).injected).count();
        assert_eq!(injected, 50);
    }
}
