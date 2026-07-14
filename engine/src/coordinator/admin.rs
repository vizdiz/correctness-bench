//! Coordinator's admin HTTP surface. POST /admin/runs takes a JSON
//! DispatchSpec; the coordinator dials each registered worker, runs the
//! slice, and returns the aggregated summary.
//!
//! This is the control-plane-facing seam. The control plane (or a CLI
//! operator) creates a run via /v1/runs, then POSTs to this endpoint to
//! actually fire it. The bench.proto gRPC surface stays internal to the
//! worker fleet.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::dispatch::{
    dispatch_with_ticks, AggregatedTick, DispatchError, DispatchSpec, DispatchSummary, Warning,
};
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
    /// Request headers (auth / custom) to send to the target. Carried to the
    /// worker over bench.proto Target.headers. Never persisted or logged.
    #[serde(default)]
    pub target_headers: std::collections::HashMap<String, String>,
    /// Shared fleet epoch (unix micros). 0/omitted = start now. Two runs given
    /// the same non-zero epoch share one schedule window (concurrent compare).
    #[serde(default)]
    pub epoch_unix_us: i64,
    pub max_latency_us: Option<i64>,
    pub min_body_bytes: Option<i32>,
    pub max_body_bytes: Option<i32>,
    pub content_type: Option<String>,
    /// Where to POST aggregated per-second ticks. Typically
    /// `http://control:8000/v1/_internal/runs/{run_id}/tick`. If omitted, no
    /// live ticks are pushed (the final summary is still returned).
    pub control_tick_url: Option<String>,
    /// Where to POST a one-shot finalize body once dispatch ends. Typically
    /// `http://control:8000/v1/_internal/runs/{run_id}/finalize`. Body shape
    /// matches the control plane's FinalizeRunRequest.
    pub control_finalize_url: Option<String>,
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
    /// 429s across the fleet — OUR load artifact, never in the correctness tally.
    pub total_rate_limited: u64,
    /// Offered RPS at which the fleet first saw a 429. `null` if never limited.
    pub rate_limit_onset_rps: Option<f64>,
    /// Workers that died mid-run (stream ended without a final Partial). 0 on a
    /// clean run. Distinct from an abort.
    pub workers_lost: usize,
    /// Non-fatal warnings (e.g. WORKER_LOST). Empty array on a clean run.
    pub warnings: Vec<Warning>,
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
        .route("/admin/runs/{id}/abort", post(abort_endpoint))
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

#[derive(Debug, Serialize)]
struct AbortResponse {
    run_id: String,
    aborted: bool,
}

/// POST /admin/runs/{id}/abort — stop an in-flight run within ~1s. Cancels the
/// local dispatch aggregation (so the run finalizes as "aborted") and fans an
/// `Abort` gRPC out to every participating worker off the request path. Returns
/// 200 immediately without blocking on the run winding down; 404 if the run is
/// not currently in flight on this coordinator.
#[tracing::instrument(name = "coordinator.abort_endpoint", level = "info", skip(state))]
async fn abort_endpoint(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<AbortResponse>, (StatusCode, Json<ErrorBody>)> {
    if state.coord.abort_run(&run_id) {
        Ok(Json(AbortResponse {
            run_id,
            aborted: true,
        }))
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorBody {
                error: format!("run {run_id} is not in flight"),
            }),
        ))
    }
}

#[tracing::instrument(
    name = "coordinator.run_endpoint",
    level = "info",
    skip(state, req),
    fields(run_id = %req.run_id, target_rps = req.target_rps, duration_s = req.duration_s),
)]
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
        target_headers: req.target_headers,
        epoch_unix_us: req.epoch_unix_us,
    };

    // Live tick aggregator. Always runs; it tracks running totals so we can
    // /finalize regardless of whether the caller wanted live push to control.
    // The closure forwards each tick to `control_tick_url` when set.
    let (tick_tx, push_handle, finals_handle) = {
        use tracing::Instrument;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AggregatedTick>();
        let tick_url = req.control_tick_url.clone();
        let finals = std::sync::Arc::new(std::sync::Mutex::new(FinalsAcc::default()));
        let finals_clone = finals.clone();
        // .in_current_span attaches the run_endpoint span to the spawned task.
        let handle = tokio::spawn(
            async move {
                let http = reqwest::Client::new();
                while let Some(tick) = rx.recv().await {
                    {
                        let mut f = finals_clone.lock().unwrap();
                        f.observe(&tick);
                    }
                    if let Some(url) = tick_url.as_ref() {
                        let req = http.post(url).json(&tick);
                        let _ = req.send().await;
                    }
                }
            }
            .in_current_span(),
        );
        (Some(tx), Some(handle), finals)
    };

    let result = dispatch_with_ticks(state.coord.clone(), spec, tick_tx).await;
    if let Some(h) = push_handle {
        let _ = h.await;
    }
    let finals_snapshot = finals_handle.lock().unwrap().clone();
    match result {
        Ok(summary) => {
            // Best-effort finalize push. Body shape matches control's
            // FinalizeRunRequest. p999 is reported as p99 (we don't carry
            // p999 in AggregatedTick today; control's column is nullable but
            // the engine binary sends it, so we set it to p99 to keep the
            // shape consistent and never zero).
            if let Some(url) = req.control_finalize_url.as_ref() {
                use base64::Engine as _;
                let enc = base64::engine::general_purpose::STANDARD;
                let hist_b64 = if finals_snapshot.last_hist_v2.is_empty() {
                    String::new()
                } else {
                    enc.encode(&finals_snapshot.last_hist_v2)
                };
                let uncorrected_b64 = if finals_snapshot.last_uncorrected_hist_v2.is_empty() {
                    String::new()
                } else {
                    enc.encode(&finals_snapshot.last_uncorrected_hist_v2)
                };
                // Honestly reduce effective_rps when workers were lost mid-run.
                // The last aggregated tick reflects the FULL fleet (ticks stall
                // once a worker stops reporting), so scale it down by the
                // survivor fraction rather than fabricating the dead worker's
                // load. On a clean run this is a no-op.
                let effective_rps = if summary.workers_lost > 0 && summary.workers_dispatched > 0 {
                    finals_snapshot.last_achieved_rps
                        * (summary.workers_finished as f64 / summary.workers_dispatched as f64)
                } else {
                    finals_snapshot.last_achieved_rps
                };
                // Warnings array (empty when none). Field name is exactly
                // "warnings" so control persists it to runs.warnings.
                let warnings: Vec<serde_json::Value> = summary
                    .warnings
                    .iter()
                    .map(|w| serde_json::json!({ "code": w.code, "message": w.message }))
                    .collect();
                let body = serde_json::json!({
                    "effective_rps":  effective_rps,
                    "p50_us":  finals_snapshot.last_p50_us as i64,
                    "p95_us":  finals_snapshot.last_p95_us as i64,
                    "p99_us":  finals_snapshot.last_p99_us as i64,
                    "p999_us": finals_snapshot.last_p99_us as i64,
                    "total_requests": summary.total_completed as i64,
                    "total_pass":     summary.total_pass as i64,
                    "cliff_rps": finals_snapshot.cliff_rps(),
                    "status": if summary.aborted { "aborted" } else { "completed" },
                    "corrected_hist_b64": hist_b64,
                    "uncorrected_hist_b64": uncorrected_b64,
                    "rate_limited": summary.total_rate_limited as i64,
                    "rate_limit_onset_rps": summary.first_429_rps,
                    "workers_lost": summary.workers_lost as i64,
                    "warnings": warnings,
                });
                let http = reqwest::Client::new();
                let req = http.post(url).json(&body);
                if let Err(e) = req.send().await {
                    eprintln!("coordinator: finalize push failed: {e}");
                }
            }
            Ok(Json(to_response(req.run_id, summary)))
        }
        Err(DispatchError::NoWorkers) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorBody { error: "no registered workers".into() }),
        )),
    }
}

#[derive(Default, Clone, Debug)]
struct FinalsAcc {
    last_p50_us: u64,
    last_p95_us: u64,
    last_p99_us: u64,
    last_achieved_rps: f64,
    // Latest non-empty V2-deflate merged HDR snapshot from any tick. Used as
    // the run-final histogram shipped to control.
    last_hist_v2: Vec<u8>,
    last_uncorrected_hist_v2: Vec<u8>,
    // Per-RPS bucket accumulator keyed by floor(rps_lo). Tracks total+pass for
    // cliff detection.
    buckets: std::collections::BTreeMap<u64, (f64, f64, u64, u64)>,
}

impl FinalsAcc {
    fn observe(&mut self, tick: &AggregatedTick) {
        // The HDR-merged percentiles are monotonically informative; take the
        // latest non-zero values so a final tick with no merge bytes doesn't
        // clobber the run finals.
        if tick.percentiles_so_far.p99_us > 0 {
            self.last_p50_us = tick.percentiles_so_far.p50_us;
            self.last_p95_us = tick.percentiles_so_far.p95_us;
            self.last_p99_us = tick.percentiles_so_far.p99_us;
        }
        if !tick.merged_hist_v2.is_empty() {
            self.last_hist_v2 = tick.merged_hist_v2.clone();
        }
        if !tick.merged_uncorrected_hist_v2.is_empty() {
            self.last_uncorrected_hist_v2 = tick.merged_uncorrected_hist_v2.clone();
        }
        self.last_achieved_rps = tick.achieved_rps_1s;
        for b in &tick.buckets {
            let entry = self.buckets.entry(b.rps_lo as u64).or_default();
            entry.0 = b.rps_lo;
            entry.1 = b.rps_hi;
            entry.2 += b.total;
            entry.3 += b.pass;
        }
    }
    fn cliff_rps(&self) -> Option<f64> {
        // Mirrors engine-worker's detector: lowest-RPS bucket with >=30 samples
        // whose pass rate fell below 0.95.
        for (_, &(lo, hi, total, pass)) in self.buckets.iter() {
            if total < 30 {
                continue;
            }
            let pass_rate = pass as f64 / total as f64;
            if pass_rate < 0.95 {
                return Some((lo + hi) / 2.0);
            }
        }
        None
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
        total_rate_limited: s.total_rate_limited,
        rate_limit_onset_rps: s.first_429_rps,
        workers_lost: s.workers_lost,
        warnings: s.warnings,
    }
}
