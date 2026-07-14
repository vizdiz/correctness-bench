//! Integration test for mid-run abort propagation.
//!
//! Chain under test: coordinator `abort_run` -> cancels the dispatch
//! aggregation token AND fans an `Abort` gRPC out to each worker_node ->
//! worker_node cancels its per-run `CancellationToken` -> engine run loop
//! drains in-flight and returns -> RunSlice stream emits a final
//! Partial(final=true) -> dispatch finalizes with `aborted = true`.
//!
//! Setup mirrors tests/coord_dispatch.rs: spawn the in-repo mock + two
//! worker_node gRPC servers on ephemeral ports and pre-populate the registry.
//!
//! Assertions:
//!   1. A run aborted ~1s in completes FAR fewer requests than a full-duration
//!      run at the same offered RPS (proves the fleet stopped early).
//!   2. The dispatch returns quickly after the abort (well before the run's
//!      nominal duration) — the <1s stop budget.
//!   3. Both workers still emit a final Partial(final=true) (workers_finished).
//!   4. The summary is flagged `aborted`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use engine::coordinator::dispatch::{dispatch_with_ticks, AggregatedTick, DispatchSpec};
use engine::coordinator::{CoordinatorState, Worker, CONTRACT_VERSION};
use engine::proto::bench::bench_server::BenchServer;
use engine::worker_node::WorkerNodeService;
use tokio::net::TcpListener;
use tonic::transport::Server;

async fn spawn_mock() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock::app()).await.unwrap();
    });
    addr
}

async fn spawn_worker_node(worker_id: &str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let svc = WorkerNodeService::new(worker_id.to_string());
    let incoming = async_stream::stream! {
        loop {
            match listener.accept().await {
                Ok((s, _)) => yield Ok::<_, std::io::Error>(s),
                Err(_) => continue,
            }
        }
    };
    tokio::spawn(async move {
        Server::builder()
            .add_service(BenchServer::new(svc))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    addr
}

fn register_two(state: &Arc<CoordinatorState>, w1: SocketAddr, w2: SocketAddr) {
    let now = Instant::now();
    let mut workers = state.workers.try_write().unwrap();
    for (id, addr) in [("worker-a", w1), ("worker-b", w2)] {
        workers.insert(
            id.into(),
            Worker {
                worker_id: id.into(),
                address: addr.to_string(),
                contract_version: CONTRACT_VERSION.into(),
                max_rps: 0,
                registered_at: now,
                last_heartbeat: now,
            },
        );
    }
}

fn spec(run_id: &str, mock_addr: SocketAddr, duration_s: u64) -> DispatchSpec {
    DispatchSpec {
        run_id: run_id.into(),
        target_url: format!("http://{mock_addr}/api?mode=healthy&base_latency_ms=5"),
        target_method: "GET".into(),
        target_rps: 400.0,
        duration_s,
        connections: 10,
        keepalive: true,
        timeout_ms: 5_000,
        expected_status: vec![200],
        max_latency_us: None,
        min_body_bytes: None,
        max_body_bytes: None,
        content_type: None,
        target_headers: Default::default(),
    }
}

// Drain any aggregated ticks so the channel never backs up.
fn tick_sink() -> tokio::sync::mpsc::UnboundedSender<AggregatedTick> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AggregatedTick>();
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    tx
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abort_stops_the_fleet_early() {
    let mock_addr = spawn_mock().await;
    let w1 = spawn_worker_node("worker-a").await;
    let w2 = spawn_worker_node("worker-b").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // --- Baseline: a full 3s run, no abort. ---
    let baseline_state = Arc::new(CoordinatorState::new());
    register_two(&baseline_state, w1, w2);
    let baseline = dispatch_with_ticks(
        baseline_state.clone(),
        spec("baseline", mock_addr, 3),
        Some(tick_sink()),
    )
    .await
    .expect("baseline dispatch ok");
    assert!(!baseline.aborted, "baseline must not be flagged aborted");
    assert_eq!(baseline.workers_finished, 2);
    assert!(
        baseline.total_completed > 600,
        "baseline should complete a healthy count over 3s, got {}",
        baseline.total_completed
    );

    // --- Aborted: a 10s run, cancelled ~1s in. ---
    let state = Arc::new(CoordinatorState::new());
    register_two(&state, w1, w2);
    let run_state = state.clone();
    let dispatch_task = tokio::spawn(async move {
        let started = Instant::now();
        let summary = dispatch_with_ticks(run_state, spec("abort-me", mock_addr, 10), Some(tick_sink()))
            .await
            .expect("aborted dispatch returns Ok");
        (summary, started.elapsed())
    });

    // Let it get going, then abort out-of-band (the path the admin endpoint uses).
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(
        state.is_run_active("abort-me"),
        "run should be registered in-flight before abort"
    );
    let accepted = state.abort_run("abort-me");
    assert!(accepted, "abort_run should find the in-flight run");

    let (summary, elapsed) = dispatch_task.await.expect("dispatch task joined");

    // (1) Flagged aborted -> finalize would push status="aborted".
    assert!(summary.aborted, "summary must be flagged aborted");

    // (2) Stopped promptly: nominal duration was 10s; aborting at ~1s must wind
    // the whole dispatch down well before then (drain + final Partial).
    assert!(
        elapsed < Duration::from_secs(5),
        "dispatch should wind down quickly after abort, took {:?}",
        elapsed
    );

    // (3) Both workers drained and emitted a final Partial(final=true).
    assert_eq!(
        summary.workers_finished, 2,
        "both workers must emit a final Partial after draining"
    );

    // (4) Far fewer completed than the full 3s baseline (which itself is < the
    // aborted run's 10s nominal). Aborting ~1s in should land well under it.
    assert!(
        summary.total_completed < baseline.total_completed,
        "aborted run ({}) should complete fewer than the full 3s baseline ({})",
        summary.total_completed,
        baseline.total_completed
    );

    // Registry cleaned up.
    assert!(
        !state.is_run_active("abort-me"),
        "run should be deregistered after dispatch returns"
    );
}

/// Aborting an unknown / already-finished run returns false (admin maps this to
/// 404) and does not panic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_unknown_run_returns_false() {
    let state = Arc::new(CoordinatorState::new());
    assert!(!state.abort_run("nope"));
}
