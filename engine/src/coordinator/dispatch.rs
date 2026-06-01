//! Dispatcher: given a run spec, build per-worker Assignments, dial each
//! registered worker's RunSlice over gRPC, and aggregate the per-worker
//! Partial streams. v0 aggregation is total counts (sum across workers);
//! the live per-tick push to control and HDR percentile merge are layered on
//! top in follow-up work.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::task::JoinHandle;
use tonic::transport::Endpoint;

use crate::coordinator::{CoordinatorState, CONTRACT_VERSION};
use crate::proto::bench::bench_client::BenchClient;
use crate::proto::bench::worker_event::Kind as WorkerEventKind;
use crate::proto::bench::{
    AssertSpec, Assignment, HttpMethod, LoadModel, RateLimitAction, RateLimitPolicy, Target,
};

/// Input to `dispatch`. Mirrors the slice of api.md's run spec the engine
/// actually needs today (no headers, body, warmup, or cost yet).
#[derive(Debug, Clone)]
pub struct DispatchSpec {
    pub run_id: String,
    pub target_url: String,
    pub target_method: String,
    pub target_rps: f64,
    pub duration_s: u64,
    pub connections: u32,
    pub keepalive: bool,
    pub timeout_ms: u32,
    pub expected_status: Vec<i32>,
    pub max_latency_us: Option<i64>,
    pub min_body_bytes: Option<i32>,
    pub max_body_bytes: Option<i32>,
    pub content_type: Option<String>,
}

/// Result of a `dispatch` call. Counts are summed across every worker that
/// participated. `workers_finished` is the count whose stream concluded with
/// a `final = true` Partial (others may have errored mid-run).
#[derive(Default, Debug)]
pub struct DispatchSummary {
    pub workers_dispatched: usize,
    pub workers_finished: usize,
    pub total_completed: u64,
    pub total_pass: u64,
    pub total_fail_status: u64,
    pub total_fail_latency: u64,
    pub total_fail_size: u64,
    pub total_fail_content_type: u64,
}

#[derive(Debug)]
pub enum DispatchError {
    NoWorkers,
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DispatchError::NoWorkers => write!(f, "no registered workers"),
        }
    }
}

impl std::error::Error for DispatchError {}

pub async fn dispatch(
    state: Arc<CoordinatorState>,
    spec: DispatchSpec,
) -> Result<DispatchSummary, DispatchError> {
    let workers = state.list_workers().await;
    if workers.is_empty() {
        return Err(DispatchError::NoWorkers);
    }

    let num_workers = workers.len();
    let per_worker_rps = spec.target_rps / num_workers as f64;
    let epoch_unix_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0);

    let mut handles: Vec<JoinHandle<WorkerSummary>> = Vec::new();
    for w in workers.iter() {
        let assignment = build_assignment(&spec, &w.worker_id, per_worker_rps, epoch_unix_us);
        let worker_url = if w.address.starts_with("http://") || w.address.starts_with("https://") {
            w.address.clone()
        } else {
            format!("http://{}", w.address)
        };
        handles.push(tokio::spawn(run_one_worker(worker_url, assignment)));
    }

    let mut summary = DispatchSummary {
        workers_dispatched: num_workers,
        ..Default::default()
    };
    for h in handles {
        let ws = match h.await {
            Ok(s) => s,
            Err(_) => continue, // task panicked
        };
        if ws.finished {
            summary.workers_finished += 1;
        }
        summary.total_completed += ws.completed;
        summary.total_pass += ws.pass;
        summary.total_fail_status += ws.fail_status;
        summary.total_fail_latency += ws.fail_latency;
        summary.total_fail_size += ws.fail_size;
        summary.total_fail_content_type += ws.fail_content_type;
    }
    Ok(summary)
}

#[derive(Default)]
struct WorkerSummary {
    finished: bool,
    completed: u64,
    pass: u64,
    fail_status: u64,
    fail_latency: u64,
    fail_size: u64,
    fail_content_type: u64,
}

async fn run_one_worker(worker_url: String, assignment: Assignment) -> WorkerSummary {
    let mut s = WorkerSummary::default();
    let endpoint = match Endpoint::from_shared(worker_url) {
        Ok(e) => e.connect_timeout(Duration::from_secs(5)),
        Err(_) => return s,
    };
    let channel = match endpoint.connect().await {
        Ok(c) => c,
        Err(_) => return s,
    };
    let mut client = BenchClient::new(channel);
    let stream = match client.run_slice(tonic::Request::new(assignment)).await {
        Ok(r) => r.into_inner(),
        Err(_) => return s,
    };
    let mut stream = stream;
    while let Ok(Some(event)) = stream.message().await {
        if let Some(WorkerEventKind::Partial(p)) = event.kind {
            for b in &p.buckets {
                s.completed += b.total;
                s.pass += b.pass;
                s.fail_status += b.fail_status;
                s.fail_latency += b.fail_latency;
                s.fail_size += b.fail_size;
                s.fail_content_type += b.fail_content_type;
            }
            if p.r#final {
                s.finished = true;
            }
        }
    }
    s
}

fn build_assignment(
    spec: &DispatchSpec,
    worker_id: &str,
    per_worker_rps: f64,
    epoch_unix_us: i64,
) -> Assignment {
    let method = match spec.target_method.to_ascii_uppercase().as_str() {
        "POST" => HttpMethod::Post as i32,
        "PUT" => HttpMethod::Put as i32,
        "PATCH" => HttpMethod::Patch as i32,
        "DELETE" => HttpMethod::Delete as i32,
        "HEAD" => HttpMethod::Head as i32,
        _ => HttpMethod::Get as i32,
    };
    Assignment {
        run_id: spec.run_id.clone(),
        worker_id: worker_id.to_string(),
        contract_version: CONTRACT_VERSION.to_string(),
        target: Some(Target {
            url: spec.target_url.clone(),
            method,
            headers: Default::default(),
            body: vec![],
            timeout_ms: spec.timeout_ms as i32,
            verify_tls: true,
        }),
        throughput: per_worker_rps,
        catch_up: per_worker_rps * 2.0,
        load_model: LoadModel::Open as i32,
        connections: spec.connections as i32,
        keepalive: spec.keepalive,
        epoch_unix_us,
        duration_s: spec.duration_s as i32,
        warmup_s: 0,
        assert: Some(AssertSpec {
            max_latency_us: spec.max_latency_us,
            min_body_bytes: spec.min_body_bytes,
            max_body_bytes: spec.max_body_bytes,
            content_type: spec.content_type.clone(),
            expected_status: spec.expected_status.clone(),
            sample_every_n: 0,
            ship_bodies: false,
            max_sampled_body_bytes: 0,
        }),
        rate_limit_policy: Some(RateLimitPolicy {
            action: RateLimitAction::RlBackoff as i32,
            max_backoff_ms: 5000,
            record_onset: true,
        }),
    }
}
