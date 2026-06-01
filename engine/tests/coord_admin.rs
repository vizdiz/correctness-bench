//! Integration test for the coordinator's admin HTTP surface.
//!
//! Spins mock + worker_node gRPC + coordinator's axum admin router on
//! ephemeral ports. Pre-populates the CoordinatorState with the worker_node's
//! address. POSTs to /admin/runs and verifies the JSON response has plausible
//! aggregate counts.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use engine::coordinator::{admin, CoordinatorState, Worker, CONTRACT_VERSION};
use engine::proto::bench::bench_server::BenchServer;
use engine::worker_node::WorkerNodeService;
use tokio::net::TcpListener;
use tonic::transport::Server as TonicServer;

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
        TonicServer::builder()
            .add_service(BenchServer::new(svc))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_runs_endpoint_dispatches_and_returns_summary() {
    let mock_addr = spawn_mock().await;
    let worker_addr = spawn_worker_node("worker-x").await;
    tokio::time::sleep(Duration::from_millis(80)).await;

    // Pre-populate the coordinator's registry.
    let state = Arc::new(CoordinatorState::new());
    let now = Instant::now();
    state.workers.write().await.insert(
        "worker-x".into(),
        Worker {
            worker_id: "worker-x".into(),
            address: worker_addr.to_string(),
            contract_version: CONTRACT_VERSION.into(),
            max_rps: 0,
            registered_at: now,
            last_heartbeat: now,
        },
    );

    // Bind the admin HTTP server on an ephemeral port.
    let admin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let admin_addr = admin_listener.local_addr().unwrap();
    let router = admin::router(state.clone());
    tokio::spawn(async move {
        axum::serve(admin_listener, router).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(40)).await;

    // POST /admin/runs against the mock at 200 RPS for 3s.
    let body = serde_json::json!({
        "run_id": "00000000-0000-0000-0000-000000000001",
        "target_url": format!("http://{mock_addr}/api?mode=healthy&base_latency_ms=5"),
        "target_rps": 200.0,
        "duration_s": 3,
        "connections": 10,
        "expected_status": [200],
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{admin_addr}/admin/runs"))
        .json(&body)
        .send()
        .await
        .expect("POST /admin/runs");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let json: serde_json::Value = resp.json().await.expect("decode JSON");
    let workers_finished = json["workers_finished"].as_u64().unwrap();
    let total_completed = json["total_completed"].as_u64().unwrap();
    let total_pass = json["total_pass"].as_u64().unwrap();
    let total_fail_status = json["total_fail_status"].as_u64().unwrap();
    assert_eq!(workers_finished, 1);
    assert!(total_completed >= 400 && total_completed <= 700, "got {total_completed}");
    assert_eq!(total_pass, total_completed);
    assert_eq!(total_fail_status, 0);

    // GET /admin/workers lists what's registered.
    let resp = client
        .get(format!("http://{admin_addr}/admin/workers"))
        .send()
        .await
        .expect("GET /admin/workers");
    let workers: serde_json::Value = resp.json().await.expect("decode workers JSON");
    assert_eq!(workers.as_array().unwrap().len(), 1);
    assert_eq!(workers[0]["worker_id"], "worker-x");
}

#[tokio::test]
async fn admin_runs_returns_503_when_no_workers() {
    let state = Arc::new(CoordinatorState::new());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let admin_addr = listener.local_addr().unwrap();
    let router = admin::router(state);
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(40)).await;

    let body = serde_json::json!({
        "run_id": "deadbeef-dead-dead-dead-deaddeadbeef",
        "target_url": "http://example.com/",
        "target_rps": 100.0,
        "duration_s": 1,
    });
    let resp = reqwest::Client::new()
        .post(format!("http://{admin_addr}/admin/runs"))
        .json(&body)
        .send()
        .await
        .expect("POST");
    assert_eq!(resp.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let json: serde_json::Value = resp.json().await.expect("decode");
    assert_eq!(json["error"], "no registered workers");
}
