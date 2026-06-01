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
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;
use tonic::{Request, Response, Status, Streaming};

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

#[derive(Default)]
pub struct CoordinatorState {
    pub workers: RwLock<HashMap<String, Worker>>,
}

impl CoordinatorState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn list_workers(&self) -> Vec<Worker> {
        self.workers.read().await.values().cloned().collect()
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
