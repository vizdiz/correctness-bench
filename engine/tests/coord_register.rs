//! Integration test for the coordinator's Register handler. Spins the gRPC
//! server in-process on an ephemeral port; dials it via a generated tonic
//! client; verifies the registry grows.

use std::sync::Arc;
use std::time::Duration;

use engine::coordinator::{CoordinatorService, CoordinatorState, CONTRACT_VERSION};
use engine::proto::bench::bench_client::BenchClient;
use engine::proto::bench::bench_server::BenchServer;
use engine::proto::bench::WorkerInfo;
use tokio::net::TcpListener;
use tonic::transport::{Endpoint, Server};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_register_grows_the_registry() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(CoordinatorState::new());
    let svc = CoordinatorService::new(state.clone());

    let incoming = async_stream::stream! {
        loop {
            match listener.accept().await {
                Ok((sock, _)) => yield Ok::<_, std::io::Error>(sock),
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

    // Give the server a beat to bind.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let channel = Endpoint::from_shared(format!("http://{}", addr))
        .expect("build endpoint")
        .connect()
        .await
        .expect("dial coord");
    let mut client = BenchClient::new(channel);

    // First worker.
    let ack = client
        .register(WorkerInfo {
            worker_id: "worker-a".into(),
            address: "127.0.0.1:9101".into(),
            contract_version: CONTRACT_VERSION.into(),
            max_rps: 5000,
        })
        .await
        .expect("register");
    let inner = ack.into_inner();
    assert!(inner.accepted, "register rejected: {}", inner.reason);
    assert_eq!(state.list_workers().await.len(), 1);

    // Second worker.
    let ack = client
        .register(WorkerInfo {
            worker_id: "worker-b".into(),
            address: "127.0.0.1:9102".into(),
            contract_version: CONTRACT_VERSION.into(),
            max_rps: 5000,
        })
        .await
        .expect("register");
    assert!(ack.into_inner().accepted);
    let workers = state.list_workers().await;
    assert_eq!(workers.len(), 2);

    // Contract version mismatch is rejected.
    let ack = client
        .register(WorkerInfo {
            worker_id: "worker-c".into(),
            address: "127.0.0.1:9103".into(),
            contract_version: "0.0.0-bad".into(),
            max_rps: 100,
        })
        .await
        .expect("register call");
    let inner = ack.into_inner();
    assert!(!inner.accepted, "old-contract worker should be rejected");
    assert!(inner.reason.contains("contract version"));
}

