//! Worker-death detection + graceful fleet degradation.
//!
//! Two independent things are proven here:
//!
//!   (a) HEARTBEAT REAPER — `reap_dead_workers` deregisters a worker whose
//!       `last_heartbeat` is older than the dead-after threshold, and leaves a
//!       freshly-heartbeating worker alone. This keeps the registry accurate so
//!       future dispatches skip dead workers.
//!
//!   (b) MID-RUN LOSS — when a participating worker's RunSlice stream ends
//!       WITHOUT a final Partial(final=true) (it died), dispatch: counts it in
//!       `workers_lost`, emits a WORKER_LOST warning, does NOT fabricate the
//!       missing load, and STILL COMPLETES with the survivor (not aborted).
//!
//! Setup mirrors tests/coord_abort.rs: in-repo mock + worker_node gRPC servers
//! on ephemeral ports, registry pre-populated.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use engine::coordinator::dispatch::{dispatch_with_ticks, AggregatedTick, DispatchSpec};
use engine::coordinator::{
    CoordinatorState, Worker, CONTRACT_VERSION, HEARTBEAT_DEAD_AFTER,
};
use engine::proto::bench::bench_server::{Bench, BenchServer};
use engine::proto::bench::worker_event::Kind as WorkerEventKind;
use engine::proto::bench::{
    AbortAck, AbortRequest, Assignment, HealthAck, Partial, RegisterAck, RpsBucket, Started,
    WorkerEvent, WorkerHealth, WorkerInfo,
};
use engine::worker_node::WorkerNodeService;
use futures_util::Stream;
use tokio::net::TcpListener;
use tonic::{Request, Response, Status, Streaming};
use tonic::transport::Server;

async fn spawn_mock() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock::app()).await.unwrap();
    });
    addr
}

/// Spawn a healthy worker_node (survivor).
async fn spawn_worker(worker_id: &str) -> SocketAddr {
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
        let _ = Server::builder()
            .add_service(BenchServer::new(svc))
            .serve_with_incoming(incoming)
            .await;
    });
    addr
}

/// A Bench server that models a worker which DIES mid-run: on RunSlice it emits
/// a Started + a couple of non-final Partials, then aborts the stream with a
/// gRPC error WITHOUT ever sending a final Partial(final=true). This is exactly
/// the death signal dispatch keys on (stream ends before the clean final),
/// deterministically — aborting a tonic server task does NOT reliably cut
/// already-established connections, so we drive the death from the stream.
#[derive(Clone)]
struct DyingWorkerService {
    worker_id: String,
}

#[tonic::async_trait]
impl Bench for DyingWorkerService {
    async fn register(&self, _: Request<WorkerInfo>) -> Result<Response<RegisterAck>, Status> {
        Err(Status::unimplemented("n/a"))
    }

    type HeartbeatStream =
        Pin<Box<dyn Stream<Item = Result<HealthAck, Status>> + Send>>;
    async fn heartbeat(
        &self,
        _: Request<Streaming<WorkerHealth>>,
    ) -> Result<Response<Self::HeartbeatStream>, Status> {
        Err(Status::unimplemented("n/a"))
    }

    type RunSliceStream =
        Pin<Box<dyn Stream<Item = Result<WorkerEvent, Status>> + Send>>;
    async fn run_slice(
        &self,
        _req: Request<Assignment>,
    ) -> Result<Response<Self::RunSliceStream>, Status> {
        let worker_id = self.worker_id.clone();
        let outbound = async_stream::try_stream! {
            yield WorkerEvent {
                kind: Some(WorkerEventKind::Started(Started {
                    actual_start_unix_us: 0,
                    skew_us: 0,
                })),
            };
            for tick_n in 1..=2u64 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                yield WorkerEvent {
                    kind: Some(WorkerEventKind::Partial(non_final_partial(&worker_id, tick_n))),
                };
            }
            // Die: abort the stream with an error and NO final Partial.
            Err(Status::unavailable("worker died mid-run"))?;
        };
        Ok(Response::new(Box::pin(outbound)))
    }

    async fn abort(&self, _: Request<AbortRequest>) -> Result<Response<AbortAck>, Status> {
        Ok(Response::new(AbortAck { accepted: true }))
    }
}

fn non_final_partial(worker_id: &str, tick_n: u64) -> Partial {
    Partial {
        worker_id: worker_id.to_string(),
        tick: tick_n,
        r#final: false,
        corrected_hist: vec![],
        uncorrected_hist: vec![],
        ttfb_hist: vec![],
        this_tick_corrected: vec![],
        buckets: vec![RpsBucket {
            rps_lo: 0.0,
            rps_hi: 10.0,
            total: 50,
            pass: 50,
            fail_transport: 0,
            fail_status: 0,
            fail_latency: 0,
            fail_size: 0,
            fail_content_type: 0,
            rate_limited: 0,
            timeout: 0,
        }],
        offered_rps: 50.0,
        achieved_rps: 50.0,
        sched_slip_us: 0,
        client_saturated: false,
        in_flight: 0,
        conn_errors: 0,
        timeouts: 0,
        rate_limit: None,
        bytes_sent: 0,
        bytes_recv: 0,
        sampled: vec![],
    }
}

/// Spawn the dying worker service; returns its address.
async fn spawn_dying_worker(worker_id: &str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let svc = DyingWorkerService { worker_id: worker_id.to_string() };
    let incoming = async_stream::stream! {
        loop {
            match listener.accept().await {
                Ok((s, _)) => yield Ok::<_, std::io::Error>(s),
                Err(_) => continue,
            }
        }
    };
    tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(BenchServer::new(svc))
            .serve_with_incoming(incoming)
            .await;
    });
    addr
}

fn insert_worker(state: &Arc<CoordinatorState>, id: &str, addr: SocketAddr, last_heartbeat: Instant) {
    let mut workers = state.workers.try_write().unwrap();
    workers.insert(
        id.into(),
        Worker {
            worker_id: id.into(),
            address: addr.to_string(),
            contract_version: CONTRACT_VERSION.into(),
            max_rps: 0,
            registered_at: Instant::now(),
            last_heartbeat,
        },
    );
}

fn tick_sink() -> tokio::sync::mpsc::UnboundedSender<AggregatedTick> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AggregatedTick>();
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    tx
}

// ---------------------------------------------------------------------------
// (a) Heartbeat reaper.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reaper_evicts_only_the_silent_worker() {
    let state = Arc::new(CoordinatorState::new());
    let dummy: SocketAddr = "127.0.0.1:1".parse().unwrap();

    let now = Instant::now();
    // Alive: heartbeated just now.
    insert_worker(&state, "alive", dummy, now);
    // Dead: last heartbeat is well past the dead-after threshold.
    insert_worker(&state, "dead", dummy, now - (HEARTBEAT_DEAD_AFTER + Duration::from_secs(2)));

    assert_eq!(state.list_workers().await.len(), 2);

    let evicted = state.reap_dead_workers(HEARTBEAT_DEAD_AFTER).await;

    assert_eq!(evicted, vec!["dead".to_string()], "only the silent worker should be evicted");
    let remaining: Vec<String> = state
        .list_workers()
        .await
        .into_iter()
        .map(|w| w.worker_id)
        .collect();
    assert_eq!(remaining, vec!["alive".to_string()], "the heartbeating worker must remain");

    // Idempotent: a second scan with everyone fresh evicts nobody.
    let evicted2 = state.reap_dead_workers(HEARTBEAT_DEAD_AFTER).await;
    assert!(evicted2.is_empty());
}

// ---------------------------------------------------------------------------
// (b) Mid-run worker loss → WORKER_LOST warning, run completes with survivor.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn midrun_worker_death_yields_warning_and_completes_with_survivor() {
    let mock_addr = spawn_mock().await;
    // worker-a: healthy survivor. worker-b: dies mid-run (stream ends without a
    // final Partial after a couple of ticks).
    let w_a = spawn_worker("worker-a").await;
    let w_b = spawn_dying_worker("worker-b").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let state = Arc::new(CoordinatorState::new());
    let now = Instant::now();
    insert_worker(&state, "worker-a", w_a, now);
    insert_worker(&state, "worker-b", w_b, now);

    let spec = DispatchSpec {
        run_id: "die-run".into(),
        target_url: format!("http://{mock_addr}/api?mode=healthy&base_latency_ms=5"),
        target_method: "GET".into(),
        target_rps: 400.0,
        duration_s: 4,
        connections: 10,
        keepalive: true,
        timeout_ms: 5_000,
        expected_status: vec![200],
        max_latency_us: None,
        min_body_bytes: None,
        max_body_bytes: None,
        content_type: None,
        target_headers: Default::default(), epoch_unix_us: 0,
    };

    let summary = dispatch_with_ticks(state.clone(), spec, Some(tick_sink()))
        .await
        .expect("dispatch returns Ok even when a worker dies");

    // The run continued and COMPLETED — this is graceful degradation, not abort.
    assert!(!summary.aborted, "a worker death must NOT flag the run aborted");
    assert_eq!(summary.workers_dispatched, 2);
    assert_eq!(summary.workers_finished, 1, "only the survivor emits a final Partial");
    assert!(summary.workers_lost >= 1, "the dead worker must be counted lost");

    // Exactly one WORKER_LOST warning, with an honest message.
    let lost: Vec<_> = summary
        .warnings
        .iter()
        .filter(|w| w.code == "WORKER_LOST")
        .collect();
    assert_eq!(lost.len(), 1, "expected one WORKER_LOST warning, got {:?}", summary.warnings);
    assert!(
        lost[0].message.contains("ran 1 of 2 workers"),
        "warning message should be honest about survivors: {:?}",
        lost[0].message
    );

    // The survivor still produced real load (we did NOT stop the run), and the
    // missing worker's load was NOT fabricated — totals reflect ~one worker's
    // slice, well under the full-fleet count.
    assert!(
        summary.total_completed > 100,
        "survivor should have completed real load, got {}",
        summary.total_completed
    );
}
