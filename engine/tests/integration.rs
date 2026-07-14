//! End-to-end integration test: spawn the mock on an ephemeral port and drive
//! the engine against it. Validates the open-loop pacing, COO ordering
//! invariant (corrected >= uncorrected), and that we account for every fired
//! request.

use std::net::SocketAddr;
use std::time::Duration;

use http::{Method, Uri};

use engine::worker::{run, run_with_ticks, RunSpec, Tick};
use tokio::sync::mpsc;

async fn spawn_mock() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock::app()).await.unwrap();
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_hits_target_rps_against_healthy_mock() {
    let addr = spawn_mock().await;
    let url = Uri::try_from(format!(
        "http://{addr}/api?mode=healthy&base_latency_ms=5"
    ))
    .unwrap();

    let spec = RunSpec {
        url,
        method: Method::GET,
        target_rps: 200.0,
        duration_s: 5,
        connections: 20,
        keepalive: true,
        timeout: Duration::from_secs(5),
        expected_status: vec![],
        max_latency_us: None,
        min_body_bytes: None,
        max_body_bytes: None,
        content_type: None,
        ramp: false,
        sample_every_n: 0,
        max_sampled_body_bytes: 0,
        rate_limit_action: engine::RlAction::Backoff,
        max_backoff_ms: 5000,
        record_onset: true,
    };

    let report = run(spec).await.expect("run completed");

    // Target was 200 rps over 5s -> ~1000 requests. Allow 10% slack for the
    // first/last partial seconds and tokio sleep granularity.
    assert!(
        report.completed >= 900 && report.completed <= 1050,
        "expected ~1000 completed, got {}",
        report.completed
    );
    assert!(
        (report.achieved_rps - 200.0).abs() < 20.0,
        "achieved rps off target: {}",
        report.achieved_rps
    );

    // Invariant: corrected >= uncorrected at every percentile (we never send
    // BEFORE intended, so wait-time only adds to corrected).
    for q in [0.5, 0.95, 0.99] {
        let c = report.histos.corrected.value_at_quantile(q);
        let u = report.histos.uncorrected.value_at_quantile(q);
        assert!(
            c >= u,
            "corrected ({}) < uncorrected ({}) at q={}",
            c,
            u,
            q
        );
    }

    // Sanity: no connection errors / timeouts on the healthy mock.
    assert_eq!(report.conn_errors, 0, "unexpected conn errors");
    assert_eq!(report.timeouts, 0, "unexpected timeouts");

    // Healthy mock returns 200 — every request should be a pass under the
    // default any-2xx rule.
    assert_eq!(report.pass, report.completed);
    assert_eq!(report.fail_status, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_tier_assertion_catches_fast500() {
    // Mock above cliff with pct=100 returns 500 on EVERY request. The engine's
    // inline status-tier assertion must flag every one as fail_status.
    let addr = spawn_mock().await;
    let url = Uri::try_from(format!(
        "http://{addr}/api?mode=fast500&cliff_rps=0&pct=100&base_latency_ms=1"
    ))
    .unwrap();

    let spec = RunSpec {
        url,
        method: Method::GET,
        target_rps: 200.0,
        duration_s: 3,
        connections: 10,
        keepalive: true,
        timeout: Duration::from_secs(3),
        expected_status: vec![200],
        max_latency_us: None,
        min_body_bytes: None,
        max_body_bytes: None,
        content_type: None,
        ramp: false,
        sample_every_n: 0,
        max_sampled_body_bytes: 0,
        rate_limit_action: engine::RlAction::Backoff,
        max_backoff_ms: 5000,
        record_onset: true,
    };

    let report = run(spec).await.expect("run completed");

    assert!(report.completed >= 500, "expected >= 500 completions, got {}", report.completed);
    assert_eq!(report.pass, 0, "no requests should pass against fast500 above cliff");
    assert_eq!(
        report.fail_status, report.completed,
        "every completed request should be classified fail_status"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn size_tier_assertion_catches_truncate() {
    // Mock above cliff in truncate mode returns a body shorter than the
    // healthy body (size + invalid JSON). The status is still 200; only the
    // size-tier assertion should fire.
    let addr = spawn_mock().await;
    let url = Uri::try_from(format!(
        "http://{addr}/api?mode=truncate&cliff_rps=0&pct=100&base_latency_ms=1"
    ))
    .unwrap();

    // Healthy body is 47 bytes; truncate body is 40 bytes. min=44 separates.
    let spec = RunSpec {
        url,
        method: Method::GET,
        target_rps: 200.0,
        duration_s: 3,
        connections: 10,
        keepalive: true,
        timeout: Duration::from_secs(3),
        expected_status: vec![200],
        max_latency_us: None,
        min_body_bytes: Some(44),
        max_body_bytes: None,
        content_type: None,
        ramp: false,
        sample_every_n: 0,
        max_sampled_body_bytes: 0,
        rate_limit_action: engine::RlAction::Backoff,
        max_backoff_ms: 5000,
        record_onset: true,
    };

    let report = run(spec).await.expect("run completed");
    assert!(report.completed >= 500, "got {} completions", report.completed);
    assert_eq!(report.fail_status, 0, "status was 200, no status fails");
    assert_eq!(
        report.fail_size, report.completed,
        "every truncated response should be fail_size; pass={} fail_size={}",
        report.pass, report.fail_size
    );
    assert_eq!(report.pass, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn latency_tier_assertion_catches_slow_ok() {
    // Mock slow_ok above cliff returns the correct body but slept ~520ms total
    // (base + slow_penalty=500). With max_latency=200ms every response is
    // fail_latency. Status is 200 and body is correct, so size/status pass.
    let addr = spawn_mock().await;
    let url = Uri::try_from(format!(
        "http://{addr}/api?mode=slow_ok&cliff_rps=0&pct=100&base_latency_ms=20"
    ))
    .unwrap();

    let spec = RunSpec {
        url,
        method: Method::GET,
        target_rps: 10.0, // keep light — slow_ok puts ~500ms per request
        duration_s: 4,
        connections: 5,
        keepalive: true,
        timeout: Duration::from_secs(5),
        expected_status: vec![200],
        max_latency_us: Some(200_000), // 200ms ceiling
        min_body_bytes: None,
        max_body_bytes: None,
        content_type: None,
        ramp: false,
        sample_every_n: 0,
        max_sampled_body_bytes: 0,
        rate_limit_action: engine::RlAction::Backoff,
        max_backoff_ms: 5000,
        record_onset: true,
    };

    let report = run(spec).await.expect("run completed");
    assert!(report.completed >= 10, "got {} completions", report.completed);
    assert_eq!(report.fail_status, 0);
    assert_eq!(report.fail_size, 0);
    assert_eq!(
        report.fail_latency, report.completed,
        "slow_ok body is correct but latency > 200ms; fail_latency={} completed={}",
        report.fail_latency, report.completed
    );
    assert_eq!(report.pass, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offload_sampling_captures_bodies_at_the_configured_cadence() {
    // Sample every 10th response; over 3 s at 200 RPS we expect roughly
    // 600/10 = 60 samples. Each sample's body is the mock's healthy JSON.
    let addr = spawn_mock().await;
    let url = Uri::try_from(format!(
        "http://{addr}/api?mode=healthy&base_latency_ms=5"
    ))
    .unwrap();

    let spec = RunSpec {
        url,
        method: Method::GET,
        target_rps: 200.0,
        duration_s: 3,
        connections: 10,
        keepalive: true,
        timeout: Duration::from_secs(3),
        expected_status: vec![200],
        max_latency_us: None,
        min_body_bytes: None,
        max_body_bytes: None,
        content_type: None,
        ramp: false,
        sample_every_n: 10,
        max_sampled_body_bytes: 4096,
        rate_limit_action: engine::RlAction::Backoff,
        max_backoff_ms: 5000,
        record_onset: true,
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<Tick>();
    let _report = run_with_ticks(spec, Some(tx)).await.expect("run completed");

    let mut total_sampled = 0usize;
    let mut any_body = String::new();
    while let Ok(t) = rx.try_recv() {
        total_sampled += t.sampled.len();
        if let Some(s) = t.sampled.first() {
            any_body = s.body.clone();
        }
    }
    assert!(
        total_sampled >= 40 && total_sampled <= 80,
        "expected ~60 samples (200rps * 3s / 10), got {total_sampled}",
    );
    assert!(
        any_body.starts_with("{\"status\":\"ok\","),
        "sampled body looks wrong: {any_body:?}",
    );
}
