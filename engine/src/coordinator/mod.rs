//! Coordinator service — implements the `Bench` gRPC service from the FROZEN
//! `contracts/bench.proto`. The coordinator handles `Register` and
//! `Heartbeat` (worker → coordinator); `RunSlice` and `Abort` are no-ops on
//! the coordinator's server because those are RPCs the coordinator CALLS on
//! workers (the proto is one service whose methods are split by role).
//!
//! State: an in-memory `{worker_id → address}` registry, per design §14.
//! No external service discovery (Consul/etcd/k8s) — workers POST register
//! on startup, heartbeats handle liveness.

pub mod admin;
pub mod dispatch;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status, Streaming};

use crate::proto::bench::bench_client::BenchClient;
use crate::proto::bench::bench_server::{Bench, BenchServer};
use crate::proto::bench::{
    AbortAck, AbortRequest, Assignment, HealthAck, RegisterAck, WorkerEvent, WorkerHealth,
    WorkerInfo,
};

/// One entry in the coordinator's worker registry.
#[derive(Debug, Clone)]
pub struct Worker {
    pub worker_id: String,
    pub address: String,
    pub contract_version: String,
    pub max_rps: u32,
    pub registered_at: Instant,
    pub last_heartbeat: Instant,
}

/// The supported `bench.proto` contract version. Workers register with their
/// own version; mismatch fails the handshake with a clear reason.
pub const CONTRACT_VERSION: &str = "1.0.0";

/// A run currently being dispatched by this coordinator. Held in the in-flight
/// registry so an out-of-band `abort_run` can (a) stop the local aggregation
/// loop and (b) fan an `Abort` gRPC out to every participating worker.
#[derive(Clone)]
pub struct RunHandle {
    /// gRPC addresses (host:port, no scheme) of the workers running this slice.
    pub worker_addrs: Vec<String>,
    /// Cancels the dispatch's aggregation loop so the run finalizes as ABORTED
    /// instead of waiting out the full duration.
    pub cancel: CancellationToken,
}

#[derive(Default)]
pub struct CoordinatorState {
    pub workers: RwLock<HashMap<String, Worker>>,
    /// Runs currently in flight, keyed by `run_id`. A `std::sync::Mutex` is
    /// enough — every critical section is a single map op, no `.await` held.
    pub runs: Mutex<HashMap<String, RunHandle>>,
}

impl CoordinatorState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn list_workers(&self) -> Vec<Worker> {
        self.workers.read().await.values().cloned().collect()
    }

    /// Register an in-flight run so it can be aborted. Called at the top of
    /// `dispatch_with_ticks`.
    pub fn register_run(&self, run_id: String, handle: RunHandle) {
        self.runs.lock().unwrap().insert(run_id, handle);
    }

    /// Deregister a run once its dispatch has wound down. Idempotent.
    pub fn deregister_run(&self, run_id: &str) {
        self.runs.lock().unwrap().remove(run_id);
    }

    /// True if `run_id` is currently in flight on this coordinator.
    pub fn is_run_active(&self, run_id: &str) -> bool {
        self.runs.lock().unwrap().contains_key(run_id)
    }

    /// Abort an in-flight run. Returns `false` if the run isn't known (already
    /// finished / never dispatched here). On success:
    ///   (a) cancels the local dispatch token so aggregation stops and the run
    ///       finalizes as ABORTED, and
    ///   (b) fires an `Abort` gRPC at every participating worker in parallel
    ///       (best-effort, short timeout) WITHOUT blocking the caller — the
    ///       fan-out runs in a detached task so the admin endpoint returns
    ///       inside the <1s budget.
    pub fn abort_run(&self, run_id: &str) -> bool {
        let handle = match self.runs.lock().unwrap().get(run_id) {
            Some(h) => h.clone(),
            None => return false,
        };
        // (a) Stop local aggregation immediately.
        handle.cancel.cancel();
        // (b) Fan Abort out to the workers off the request path.
        let run_id_owned = run_id.to_string();
        tokio::spawn(async move {
            fan_out_aborts(run_id_owned, handle.worker_addrs).await;
        });
        true
    }
}

/// Dial each worker's `Abort` gRPC in parallel, best-effort, with a short
/// per-worker timeout. Failures are logged and ignored — the local token cancel
/// already guarantees the run finalizes as aborted; the gRPC is what makes the
/// workers stop firing load within ~1s.
async fn fan_out_aborts(run_id: String, worker_addrs: Vec<String>) {
    let mut tasks = Vec::new();
    for addr in worker_addrs {
        let run_id = run_id.clone();
        tasks.push(tokio::spawn(async move {
            let url = if addr.starts_with("http://") || addr.starts_with("https://") {
                addr.clone()
            } else {
                format!("http://{addr}")
            };
            let endpoint = match tonic::transport::Endpoint::from_shared(url) {
                Ok(e) => e.connect_timeout(Duration::from_millis(500)),
                Err(_) => return,
            };
            let channel = match tokio::time::timeout(Duration::from_millis(500), endpoint.connect())
                .await
            {
                Ok(Ok(c)) => c,
                _ => {
                    eprintln!("coordinator: abort dial failed for {addr}");
                    return;
                }
            };
            let mut client = BenchClient::new(channel);
            let req = tonic::Request::new(AbortRequest {
                run_id: run_id.clone(),
                worker_id: String::new(), // empty = this run on that worker
                reason: "operator abort".to_string(),
            });
            match tokio::time::timeout(Duration::from_millis(500), client.abort(req)).await {
                Ok(Ok(_)) => {}
                _ => eprintln!("coordinator: abort rpc failed for {addr}"),
            }
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
}

#[derive(Clone)]
pub struct CoordinatorService {
    pub state: Arc<CoordinatorState>,
}

impl CoordinatorService {
    pub fn new(state: Arc<CoordinatorState>) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl Bench for CoordinatorService {
    async fn register(&self, req: Request<WorkerInfo>) -> Result<Response<RegisterAck>, Status> {
        let info = req.into_inner();
        if info.contract_version != CONTRACT_VERSION {
            return Ok(Response::new(RegisterAck {
                accepted: false,
                reason: format!(
                    "contract version mismatch: worker={} coordinator={}",
                    info.contract_version, CONTRACT_VERSION,
                ),
            }));
        }
        if info.worker_id.is_empty() || info.address.is_empty() {
            return Ok(Response::new(RegisterAck {
                accepted: false,
                reason: "worker_id and address are required".to_string(),
            }));
        }
        let now = Instant::now();
        let worker = Worker {
            worker_id: info.worker_id.clone(),
            address: info.address.clone(),
            contract_version: info.contract_version.clone(),
            max_rps: info.max_rps,
            registered_at: now,
            last_heartbeat: now,
        };
        let mut guard = self.state.workers.write().await;
        let replaced = guard.insert(info.worker_id.clone(), worker).is_some();
        let count = guard.len();
        drop(guard);
        eprintln!(
            "coordinator: {} worker {} @ {} (now {} registered)",
            if replaced { "re-registered" } else { "registered" },
            info.worker_id,
            info.address,
            count,
        );
        Ok(Response::new(RegisterAck {
            accepted: true,
            reason: String::new(),
        }))
    }

    type HeartbeatStream =
        std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<HealthAck, Status>> + Send>>;

    async fn heartbeat(
        &self,
        req: Request<Streaming<WorkerHealth>>,
    ) -> Result<Response<Self::HeartbeatStream>, Status> {
        let mut inbound = req.into_inner();
        let state = self.state.clone();
        let stream = async_stream::try_stream! {
            while let Some(msg) = inbound.message().await? {
                let now = Instant::now();
                let mut workers = state.workers.write().await;
                if let Some(w) = workers.get_mut(&msg.worker_id) {
                    w.last_heartbeat = now;
                }
                drop(workers);
                yield HealthAck { ok: true };
            }
        };
        Ok(Response::new(Box::pin(stream) as Self::HeartbeatStream))
    }

    // RunSlice and Abort are RPCs the COORDINATOR calls on WORKERS — the
    // coordinator's own server doesn't handle them.
    type RunSliceStream =
        std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<WorkerEvent, Status>> + Send>>;

    async fn run_slice(
        &self,
        _req: Request<Assignment>,
    ) -> Result<Response<Self::RunSliceStream>, Status> {
        Err(Status::unimplemented(
            "RunSlice is implemented on the worker side, not the coordinator",
        ))
    }

    async fn abort(&self, _req: Request<AbortRequest>) -> Result<Response<AbortAck>, Status> {
        Err(Status::unimplemented(
            "Abort is implemented on the worker side, not the coordinator",
        ))
    }
}

/// Bind a tonic server with the coordinator's Bench impl on `addr`. Future
/// resolves only when the server stops (signal, error, etc.). Returns the
/// shared state so the caller can introspect (CLI / tests).
pub async fn serve(addr: SocketAddr) -> Result<Arc<CoordinatorState>, Box<dyn std::error::Error>> {
    let state = Arc::new(CoordinatorState::new());
    let svc = CoordinatorService::new(state.clone());
    eprintln!("coordinator: listening on {}", addr);
    tonic::transport::Server::builder()
        .add_service(BenchServer::new(svc))
        .serve(addr)
        .await?;
    Ok(state)
}
