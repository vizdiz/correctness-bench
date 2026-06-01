//! Coordinator's admin HTTP surface. POST /admin/runs takes a JSON
//! DispatchSpec; the coordinator dials each registered worker, runs the
//! slice, and returns the aggregated summary.
//!
//! This is the control-plane-facing seam. The control plane (or a CLI
//! operator) creates a run via /v1/runs, then POSTs to this endpoint to
//! actually fire it. The bench.proto gRPC surface stays internal to the
//! worker fleet.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::dispatch::{dispatch_with_ticks, AggregatedTick, DispatchError, DispatchSpec, DispatchSummary};
use super::{CoordinatorState, Worker};

#[derive(Clone)]
struct AppState {
    coord: Arc<CoordinatorState>,
}

/// JSON shape POSTed to /admin/runs. Field names match DispatchSpec for
/// painless mapping.
#[derive(Debug, Deserialize)]
pub struct RunRequest {
    pub run_id: String,
    pub target_url: String,
    #[serde(default = "default_method")]
    pub target_method: String,
    pub target_rps: f64,
    pub duration_s: u64,
    #[serde(default = "default_connections")]
    pub connections: u32,
    #[serde(default = "default_keepalive")]
    pub keepalive: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u32,
    #[serde(default)]
    pub expected_status: Vec<i32>,
    pub max_latency_us: Option<i64>,
    pub min_body_bytes: Option<i32>,
    pub max_body_bytes: Option<i32>,
    pub content_type: Option<String>,
    /// Where to POST aggregated per-second ticks. Typically
    /// `http://control:8000/v1/_internal/runs/{run_id}/tick`. If omitted, no
    /// live ticks are pushed (the final summary is still returned).
    pub control_tick_url: Option<String>,
}

fn default_method() -> String { "GET".into() }
fn default_connections() -> u32 { 50 }
fn default_keepalive() -> bool { true }
fn default_timeout_ms() -> u32 { 30_000 }

#[derive(Debug, Serialize)]
pub struct RunResponse {
    pub run_id: String,
    pub workers_dispatched: usize,
    pub workers_finished: usize,
    pub total_completed: u64,
    pub total_pass: u64,
    pub total_fail_status: u64,
    pub total_fail_latency: u64,
    pub total_fail_size: u64,
    pub total_fail_content_type: u64,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Debug, Serialize)]
struct WorkerView {
    worker_id: String,
    address: String,
    contract_version: String,
    max_rps: u32,
}

impl From<&Worker> for WorkerView {
    fn from(w: &Worker) -> Self {
        Self {
            worker_id: w.worker_id.clone(),
            address: w.address.clone(),
            contract_version: w.contract_version.clone(),
            max_rps: w.max_rps,
        }
    }
}

pub fn router(coord: Arc<CoordinatorState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/admin/workers", get(list_workers))
        .route("/admin/runs", post(run_endpoint))
        .with_state(AppState { coord })
}

async fn healthz() -> &'static str {
    "ok"
}

async fn list_workers(State(state): State<AppState>) -> impl IntoResponse {
    let workers = state.coord.list_workers().await;
    let views: Vec<WorkerView> = workers.iter().map(WorkerView::from).collect();
    Json(views)
}

async fn run_endpoint(
    State(state): State<AppState>,
    Json(req): Json<RunRequest>,
) -> Result<Json<RunResponse>, (StatusCode, Json<ErrorBody>)> {
    let spec = DispatchSpec {
        run_id: req.run_id.clone(),
        target_url: req.target_url,
        target_method: req.target_method,
        target_rps: req.target_rps,
        duration_s: req.duration_s,
        connections: req.connections,
        keepalive: req.keepalive,
        timeout_ms: req.timeout_ms,
        expected_status: req.expected_status,
        max_latency_us: req.max_latency_us,
        min_body_bytes: req.min_body_bytes,
        max_body_bytes: req.max_body_bytes,
        content_type: req.content_type,
    };

    // Optional live tick pusher: if a control_tick_url is supplied, drain
    // aggregated ticks and POST each as JSON.
    let (tick_tx, push_handle) = match req.control_tick_url.as_ref() {
        Some(url) => {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AggregatedTick>();
            let url = url.clone();
            let handle = tokio::spawn(async move {
                let http = reqwest::Client::new();
                while let Some(tick) = rx.recv().await {
                    let _ = http.post(&url).json(&tick).send().await;
                }
            });
            (Some(tx), Some(handle))
        }
        None => (None, None),
    };

    let result = dispatch_with_ticks(state.coord.clone(), spec, tick_tx).await;
    if let Some(h) = push_handle {
        let _ = h.await;
    }
    match result {
        Ok(summary) => Ok(Json(to_response(req.run_id, summary))),
        Err(DispatchError::NoWorkers) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorBody { error: "no registered workers".into() }),
        )),
    }
}

fn to_response(run_id: String, s: DispatchSummary) -> RunResponse {
    RunResponse {
        run_id,
        workers_dispatched: s.workers_dispatched,
        workers_finished: s.workers_finished,
        total_completed: s.total_completed,
        total_pass: s.total_pass,
        total_fail_status: s.total_fail_status,
        total_fail_latency: s.total_fail_latency,
        total_fail_size: s.total_fail_size,
        total_fail_content_type: s.total_fail_content_type,
    }
}
