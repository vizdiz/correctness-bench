//! Integration test for the worker_node's RunSlice handler.
//!
//! Setup: spin the mock on an ephemeral port; bind a worker_node gRPC server
//! on another ephemeral port. Dial the worker_node, send `RunSlice(Assignment)`
//! pointing at the mock. Assert that we receive `Started` + at least one
//! `Partial` per second + a final `Partial { final = true }`, and that the
//! buckets' counts add up to a non-trivial number of requests.

use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use engine::proto::bench::bench_client::BenchClient;
use engine::proto::bench::bench_server::BenchServer;
use engine::proto::bench::worker_event::Kind as WorkerEventKind;
use engine::proto::bench::{
    AssertSpec, Assignment, HttpMethod, LoadModel, RateLimitAction, RateLimitPolicy, Target,
};
use engine::worker_node::WorkerNodeService;
use tokio::net::TcpListener;
use tonic::transport::{Endpoint, Server};

async fn spawn_mock() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock::app()).await.unwrap();
    });
    addr
}

async fn spawn_worker_node() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let svc = WorkerNodeService::new("test-worker".into());
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
async fn worker_node_runs_slice_against_healthy_mock() {
    let mock_addr = spawn_mock().await;
    let worker_addr = spawn_worker_node().await;
    tokio::time::sleep(Duration::from_millis(80)).await;

    let channel = Endpoint::from_shared(format!("http://{worker_addr}"))
        .unwrap()
        .connect()
        .await
        .expect("dial worker_node");
    let mut client = BenchClient::new(channel);

    let epoch_unix_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0);

    let assignment = Assignment {
        run_id: "00000000-0000-0000-0000-000000000001".into(),
        worker_id: "test-worker".into(),
        contract_version: engine::coordinator::CONTRACT_VERSION.into(),
        target: Some(Target {
            url: format!("http://{mock_addr}/api?mode=healthy&base_latency_ms=5"),
            method: HttpMethod::Get as i32,
            headers: Default::default(),
            body: vec![],
            timeout_ms: 5000,
            verify_tls: false,
        }),
        throughput: 200.0,
        catch_up: 400.0,
        load_model: LoadModel::Open as i32,
        connections: 10,
        keepalive: true,
        epoch_unix_us,
        duration_s: 3,
        warmup_s: 0,
        assert: Some(AssertSpec {
            max_latency_us: None,
            min_body_bytes: None,
            max_body_bytes: None,
            content_type: None,
            expected_status: vec![200],
            sample_every_n: 0,
            ship_bodies: false,
            max_sampled_body_bytes: 0,
        }),
        rate_limit_policy: Some(RateLimitPolicy {
            action: RateLimitAction::RlBackoff as i32,
            max_backoff_ms: 5000,
            record_onset: true,
        }),
    };

    let stream = client
        .run_slice(tonic::Request::new(assignment))
        .await
        .expect("run_slice")
        .into_inner();

    let mut stream = stream;
    let mut started = false;
    let mut partials = 0u32;
    let mut total_in_buckets = 0u64;
    let mut saw_final = false;

    loop {
        let next = tokio::time::timeout(Duration::from_secs(10), stream.message())
            .await
            .expect("stream message timeout")
            .expect("stream rpc error");
        let Some(event) = next else { break };
        match event.kind.expect("event must have a kind") {
            WorkerEventKind::Started(_) => {
                started = true;
            }
            WorkerEventKind::Partial(p) => {
                partials += 1;
                for b in &p.buckets {
                    total_in_buckets += b.total;
                }
                if p.r#final {
                    saw_final = true;
                    break;
                }
            }
            other => {
                panic!("unexpected event {other:?}");
            }
        }
    }

    assert!(started, "no Started event observed");
    assert!(saw_final, "no final Partial observed");
    assert!(partials >= 2, "expected several Partials, got {partials}");
    assert!(
        total_in_buckets >= 400,
        "expected ~600 reqs across 3s @ 200rps, bucket total = {total_in_buckets}"
    );
}
