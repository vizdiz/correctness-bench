//! End-to-end integration test: spawn the mock on an ephemeral port and drive
//! the engine against it. Validates the open-loop pacing, COO ordering
//! invariant (corrected >= uncorrected), and that we account for every fired
//! request.

use std::net::SocketAddr;
use std::time::Duration;

use engine::worker::{run, RunSpec};
use reqwest::{Method, Url};

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
    let url = Url::parse(&format!(
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
    };

    let report = run(spec).await.expect("run completed");

    // Target was 200 rps over 5s -> ~1000 requests. Allow 5% slack for the
    // first/last partial seconds and tokio sleep granularity.
    assert!(
        report.completed >= 950 && report.completed <= 1050,
        "expected ~1000 completed, got {}",
        report.completed
    );
    assert!(
        (report.achieved_rps - 200.0).abs() < 10.0,
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
}
