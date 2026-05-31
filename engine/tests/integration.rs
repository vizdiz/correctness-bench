//! End-to-end integration test: spawn the mock on an ephemeral port and drive
//! the engine against it. Validates the open-loop pacing, COO ordering
//! invariant (corrected >= uncorrected), and that we account for every fired
//! request.

use std::net::SocketAddr;
use std::time::Duration;

use http::{Method, Uri};

use engine::worker::{run, RunSpec};

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
        ramp: false,
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
        ramp: false,
    };

    let report = run(spec).await.expect("run completed");

    assert!(report.completed >= 500, "expected >= 500 completions, got {}", report.completed);
    assert_eq!(report.pass, 0, "no requests should pass against fast500 above cliff");
    assert_eq!(
        report.fail_status, report.completed,
        "every completed request should be classified fail_status"
    );
}
