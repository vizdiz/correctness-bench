//! Integration test for the dispatcher.
//!
//! Setup: spawn the mock + TWO worker_node gRPC servers on ephemeral ports.
//! Pre-populate a CoordinatorState with both as registered workers (skipping
//! the Register round-trip - covered by tests/coord_register.rs). Call
//! `coordinator::dispatch::dispatch`. Verify (a) the aggregated total
//! requests roughly match `target_rps * duration_s`, (b) both workers
//! finished, (c) all pass = total (healthy mock).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use engine::coordinator::dispatch::{dispatch, DispatchSpec};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_aggregates_across_two_workers() {
    let mock_addr = spawn_mock().await;
    let w1_addr = spawn_worker_node("worker-a").await;
    let w2_addr = spawn_worker_node("worker-b").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Pre-populate the coordinator's registry directly.
    let state = Arc::new(CoordinatorState::new());
    let now = Instant::now();
    let mut workers = state.workers.write().await;
    workers.insert(
        "worker-a".into(),
        Worker {
            worker_id: "worker-a".into(),
            address: w1_addr.to_string(),
            contract_version: CONTRACT_VERSION.into(),
            max_rps: 0,
            registered_at: now,
            last_heartbeat: now,
        },
    );
    workers.insert(
        "worker-b".into(),
        Worker {
            worker_id: "worker-b".into(),
            address: w2_addr.to_string(),
            contract_version: CONTRACT_VERSION.into(),
            max_rps: 0,
            registered_at: now,
            last_heartbeat: now,
        },
    );
    drop(workers);

    // Sanity that HashMap import is wired (silence unused).
    let _: HashMap<&str, &str> = HashMap::new();

    let spec = DispatchSpec {
        run_id: "test-run".into(),
        target_url: format!("http://{mock_addr}/api?mode=healthy&base_latency_ms=5"),
        target_method: "GET".into(),
        target_rps: 400.0,
        duration_s: 3,
        connections: 10,
        keepalive: true,
        timeout_ms: 5_000,
        expected_status: vec![200],
        max_latency_us: None,
        min_body_bytes: None,
        max_body_bytes: None,
        content_type: None,
    };

    let summary = dispatch(state.clone(), spec)
        .await
        .expect("dispatch ok");

    assert_eq!(summary.workers_dispatched, 2);
    assert_eq!(summary.workers_finished, 2, "both workers should have finished");
    // 400 rps * 3 s ≈ 1200 total (allow generous slack for warmup + final-tick truncation).
    assert!(
        summary.total_completed >= 800 && summary.total_completed <= 1400,
        "expected ~1200 completed across workers, got {}",
        summary.total_completed
    );
    // Healthy mock: every request should pass.
    assert_eq!(
        summary.total_pass, summary.total_completed,
        "pass should equal completed against healthy mock"
    );
    assert_eq!(summary.total_fail_status, 0);
}
