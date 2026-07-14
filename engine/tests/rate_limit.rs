//! Rate-limit (HTTP 429) handling tests.
//!
//! Core thesis check: a 429 is OUR load artifact (we out-ran the target's rate
//! limiter), NEVER folded into the API's correctness score. These tests prove:
//!   1. A 429 is counted as `rate_limited`, not `fail_status` (or any fail_*).
//!   2. The correctness pass-rate is unaffected by 429s (they are outside the
//!      denominator).
//!   3. `first_429_rps` onset is surfaced.
//!   4. RL_CONTINUE keeps firing; RL_ABORT stops the run.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use http::{Method, Uri};

use engine::worker::{run, RlAction, RunSpec};

/// A tiny target that returns 429 for the first `n_429` requests (with a
/// `Retry-After: 0` header so backoff is a no-op we can still observe), then
/// 200 (healthy JSON body) forever after. Deterministic, no load dependence.
#[derive(Clone)]
struct MockState {
    seen: Arc<AtomicU64>,
    n_429: u64,
    retry_after: &'static str,
}

async fn handler(State(st): State<MockState>) -> axum::response::Response {
    let n = st.seen.fetch_add(1, Ordering::Relaxed);
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "application/json".parse().unwrap());
    if n < st.n_429 {
        headers.insert("retry-after", st.retry_after.parse().unwrap());
        let body = r#"{"status":"error","code":429}"#;
        (StatusCode::TOO_MANY_REQUESTS, headers, body).into_response()
    } else {
        let body = r#"{"status":"ok"}"#;
        (StatusCode::OK, headers, body).into_response()
    }
}

async fn spawn_429_mock(n_429: u64, retry_after: &'static str) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = MockState {
        seen: Arc::new(AtomicU64::new(0)),
        n_429,
        retry_after,
    };
    let app = Router::new()
        .route("/api", get(handler))
        .with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn spec(url: Uri, action: RlAction) -> RunSpec {
    RunSpec {
        url,
        method: Method::GET,
        target_rps: 200.0,
        duration_s: 3,
        connections: 10,
        keepalive: true,
        timeout: Duration::from_secs(3),
        // Require 200 explicitly: proves a 429 is NOT scored fail_status even
        // when 429 is not in the expected set.
        expected_status: vec![200],
        max_latency_us: None,
        min_body_bytes: None,
        max_body_bytes: None,
        content_type: None,
        ramp: false,
        sample_every_n: 0,
        max_sampled_body_bytes: 0,
        rate_limit_action: action,
        max_backoff_ms: 5000,
        record_onset: true,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_429_is_rate_limited_not_fail_status() {
    // First 200 responses across all connections are 429; the rest are 200.
    let addr = spawn_429_mock(200, "0").await;
    let url = Uri::try_from(format!("http://{addr}/api")).unwrap();

    // RL_CONTINUE so the run keeps firing straight through the 429 band and we
    // get a clean count of both 429s and subsequent 200 passes.
    let report = run(spec(url, RlAction::Continue)).await.expect("run completed");

    // We saw the 429s.
    assert!(
        report.rate_limited >= 190,
        "expected ~200 rate_limited, got {}",
        report.rate_limited
    );

    // The core invariant: a 429 is NEVER a fail_status (or any fail_* tier).
    assert_eq!(report.fail_status, 0, "429 leaked into fail_status");
    assert_eq!(report.fail_latency, 0, "429 leaked into fail_latency");
    assert_eq!(report.fail_size, 0, "429 leaked into fail_size");
    assert_eq!(report.fail_content_type, 0, "429 leaked into fail_content_type");

    // Accounting closes: every completed response is exactly one of
    // pass / fail_* / rate_limited.
    let classified = report.pass
        + report.fail_status
        + report.fail_latency
        + report.fail_size
        + report.fail_content_type
        + report.rate_limited;
    assert_eq!(
        classified, report.completed,
        "classification does not partition completed: {classified} != {}",
        report.completed
    );

    // Pass-rate is UNAFFECTED by 429s: the denominator excludes them, and every
    // non-429 response here is a healthy 200 → 100% correctness.
    let denom = report.completed - report.rate_limited;
    assert!(denom > 0, "expected some non-429 responses");
    assert_eq!(
        report.pass, denom,
        "every non-429 response should pass: pass={} denom={}",
        report.pass, denom
    );

    // Onset is surfaced (offered RPS at the first 429; constant schedule → ~200).
    let onset = report.first_429_rps.expect("first_429_rps should be set");
    assert!(
        (onset - 200.0).abs() < 1.0,
        "onset offered-rps should equal target 200, got {onset}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_429_yields_zero_correctness_denominator_not_zero_passrate_failure() {
    // Every response is a 429. Correctness denominator is 0 (no non-429
    // responses), so pass-rate is undefined — NOT a 0% correctness collapse.
    let addr = spawn_429_mock(u64::MAX, "0").await;
    let url = Uri::try_from(format!("http://{addr}/api")).unwrap();

    let report = run(spec(url, RlAction::Continue)).await.expect("run completed");

    assert!(report.completed > 0, "should have completed some requests");
    assert_eq!(report.rate_limited, report.completed, "all responses were 429");
    assert_eq!(report.fail_status, 0, "no 429 may be scored as fail_status");
    assert_eq!(report.pass, 0);
    // Correctness denominator collapses to zero — the API never actually failed.
    assert_eq!(report.completed - report.rate_limited, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rl_backoff_honors_retry_after_capped_and_keeps_coo_honest() {
    // First 10 responses are 429 with `Retry-After: 1` (1000 ms). With backoff
    // capped at 50 ms, each honored 429 accrues 50 ms, and the schedule origin
    // is shifted forward so the deferred requests are NOT charged the wait as
    // (target) latency — corrected latency stays sane.
    let addr = spawn_429_mock(10, "1").await;
    let url = Uri::try_from(format!("http://{addr}/api")).unwrap();

    let mut s = spec(url, RlAction::Backoff);
    s.max_backoff_ms = 50; // cap honoring well below the advertised 1000 ms
    s.connections = 2; // fewer conns → the 10 429s are spread over the run

    let report = run(s).await.expect("run completed");

    assert!(report.rate_limited >= 10, "expected the 429 band, got {}", report.rate_limited);
    assert_eq!(report.fail_status, 0, "429 must never be fail_status");

    // Retry-After was surfaced (1000 ms as advertised, before capping).
    assert_eq!(
        report.last_retry_after_ms,
        Some(1000),
        "last Retry-After should be the advertised 1000 ms"
    );

    // Backoff time accrued, and it is bounded by (429s honored) * cap. Each of
    // the 10 429s honors at most 50 ms.
    assert!(report.backoff_us_total > 0, "backoff should have accrued");
    assert!(
        report.backoff_us_total <= report.rate_limited * 50 * 1000,
        "backoff {} us exceeds cap * 429count",
        report.backoff_us_total
    );

    // COO honesty: because we shift the schedule origin over the backoff, the
    // corrected p99 must NOT balloon to ~1s (the advertised Retry-After). The
    // target itself is fast; corrected p99 should stay well under 500 ms.
    let p99 = report.corrected.p99_us;
    assert!(
        p99 < 500_000,
        "corrected p99 {} us looks like backoff leaked into latency (COO dishonest)",
        p99
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rl_abort_stops_the_run_early() {
    // Every response is a 429; RL_ABORT should stop the run well before the
    // full 3s * 200rps = 600 requests would complete.
    let addr = spawn_429_mock(u64::MAX, "0").await;
    let url = Uri::try_from(format!("http://{addr}/api")).unwrap();

    let report = run(spec(url, RlAction::Abort)).await.expect("run completed");

    assert!(report.rate_limited >= 1, "should have seen at least one 429");
    // With 10 connections aborting on their first 429, we expect far fewer than
    // a full run's worth of completions.
    assert!(
        report.completed < 200,
        "RL_ABORT should have stopped early, got {} completions",
        report.completed
    );
}
