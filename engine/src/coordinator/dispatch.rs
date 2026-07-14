//! Dispatcher: given a run spec, build per-worker Assignments, dial each
//! registered worker's RunSlice over gRPC, and aggregate the per-worker
//! Partial streams. v0 aggregation is total counts (sum across workers);
//! the live per-tick push to control and HDR percentile merge are layered on
//! top in follow-up work.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tonic::transport::Endpoint;

use crate::coordinator::{CoordinatorState, CONTRACT_VERSION};
use crate::proto::bench::bench_client::BenchClient;
use crate::proto::bench::worker_event::Kind as WorkerEventKind;
use crate::proto::bench::{
    AssertSpec, Assignment, HttpMethod, LoadModel, Partial, RateLimitAction, RateLimitPolicy,
    Target,
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
    /// Request headers (auth / custom) carried to the target via
    /// bench.proto Target.headers. Never persisted or logged by anyone.
    pub target_headers: std::collections::HashMap<String, String>,
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
    /// Total 429s across the fleet. OUR load artifact — never folded into the
    /// correctness totals above.
    pub total_rate_limited: u64,
    /// Minimum non-null onset across workers: the lowest offered-RPS-at-send at
    /// which any worker first saw a 429. `None` if no worker was rate limited.
    pub first_429_rps: Option<f64>,
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

/// One per-second aggregated tick, summed across every worker that reported
/// at that tick number. Shape mirrors the JSON the control plane's tick broker
/// already accepts (engine -> POST /v1/_internal/runs/:id/tick) so the
/// coordinator can pipe these straight into the same surface.
///
/// `percentiles_so_far` is merged across every worker's V2-deflate HDR
/// snapshot (`Partial.corrected_hist`) for this tick. If a worker's bytes are
/// missing or malformed, that worker is excluded from the merge for that tick;
/// the field falls back to {0,0} when no worker shipped a parseable snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct AggregatedTick {
    pub elapsed_s: u64,
    pub achieved_rps_1s: f64,
    pub completed_total: u64,
    pub pass_total: u64,
    pub fail_status_total: u64,
    pub this_tick: AggThisTick,
    pub percentiles_so_far: AggPercentiles,
    pub buckets: Vec<AggBucket>,
    /// Offered RPS at which the fleet first saw a 429 (minimum non-null onset
    /// across workers). `null` until some worker is rate limited.
    pub rate_limit_onset_rps: Option<f64>,
    /// V2-deflate of the merged corrected HDR for this tick across all
    /// workers. Skipped from JSON (control gets `percentiles_so_far` and the
    /// raw bytes ride only on the in-process channel + the admin /finalize
    /// push). Empty when no worker shipped a parseable snapshot this tick.
    #[serde(skip, default)]
    pub merged_hist_v2: Vec<u8>,
    /// V2-deflate of the merged uncorrected HDR for this tick. Same
    /// transport rules as `merged_hist_v2`; used to ship the COO-delta on
    /// finalized fleet runs.
    #[serde(skip, default)]
    pub merged_uncorrected_hist_v2: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AggThisTick {
    pub total: u64,
    pub pass: u64,
    pub fail_status: u64,
    pub fail_latency: u64,
    pub fail_size: u64,
    pub fail_content_type: u64,
    /// 429s across the fleet this tick. Outside the correctness denominator.
    pub rate_limited: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AggPercentiles {
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AggBucket {
    pub rps_lo: f64,
    pub rps_hi: f64,
    pub total: u64,
    pub pass: u64,
    pub fail_status: u64,
    pub fail_latency: u64,
    pub fail_size: u64,
    pub fail_content_type: u64,
    /// 429s in this aggregated bucket. Excluded from the correctness
    /// denominator (`total - rate_limited`).
    pub rate_limited: u64,
}

/// Dispatch a run without live ticks. Returns the final aggregated totals.
pub async fn dispatch(
    state: Arc<CoordinatorState>,
    spec: DispatchSpec,
) -> Result<DispatchSummary, DispatchError> {
    dispatch_with_ticks(state, spec, None).await
}

/// Dispatch a run with optional live per-tick aggregation streamed via `tick_tx`.
/// Emits one [`AggregatedTick`] per global tick number once EVERY worker has
/// reported that tick. (If a worker dies mid-run, ticks past its last report
/// are silently dropped; the final per-worker totals still go into the
/// returned [`DispatchSummary`] via each worker task's own counts.)
#[tracing::instrument(
    name = "coordinator.dispatch",
    level = "info",
    skip(state, spec, tick_tx),
    fields(
        run_id = %spec.run_id,
        target_rps = spec.target_rps,
        duration_s = spec.duration_s,
    ),
)]
pub async fn dispatch_with_ticks(
    state: Arc<CoordinatorState>,
    spec: DispatchSpec,
    tick_tx: Option<UnboundedSender<AggregatedTick>>,
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

    // Each worker task sends (worker_id, Partial) into this channel.
    let (partial_tx, partial_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, Partial)>();

    let mut handles: Vec<JoinHandle<WorkerSummary>> = Vec::new();
    for w in workers.iter() {
        let assignment = build_assignment(&spec, &w.worker_id, per_worker_rps, epoch_unix_us);
        let worker_url = if w.address.starts_with("http://") || w.address.starts_with("https://") {
            w.address.clone()
        } else {
            format!("http://{}", w.address)
        };
        let tx = partial_tx.clone();
        let worker_id = w.worker_id.clone();
        handles.push(tokio::spawn(run_one_worker(worker_url, worker_id, assignment, tx)));
    }
    drop(partial_tx);

    // Live per-tick aggregator.
    aggregate_loop(partial_rx, tick_tx, num_workers).await;

    // Final per-tier totals: sum each worker task's own counts (independent of
    // the per-tick aggregation, which may drop ticks if a worker dies mid-run).
    let mut summary = DispatchSummary {
        workers_dispatched: num_workers,
        ..Default::default()
    };
    for h in handles {
        let ws = match h.await {
            Ok(s) => s,
            Err(_) => continue,
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
        summary.total_rate_limited += ws.rate_limited;
        if let Some(onset) = ws.first_429_rps {
            summary.first_429_rps = Some(match summary.first_429_rps {
                Some(cur) => cur.min(onset),
                None => onset,
            });
        }
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
    rate_limited: u64,
    /// This worker's onset (offered-RPS-at-send of its first 429), carried up
    /// from `Partial.rate_limit.first_429_rps`. `None` if never rate limited.
    first_429_rps: Option<f64>,
}

#[tracing::instrument(
    name = "coordinator.run_one_worker",
    level = "info",
    skip(assignment, partial_tx),
    fields(worker_id, worker_url, run_id = %assignment.run_id),
)]
async fn run_one_worker(
    worker_url: String,
    worker_id: String,
    assignment: Assignment,
    partial_tx: tokio::sync::mpsc::UnboundedSender<(String, Partial)>,
) -> WorkerSummary {
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
                s.rate_limited += b.rate_limited;
            }
            // Carry the onset from the rate-limit signal (run-wide, so every
            // tick after the first 429 repeats it). Keep the minimum.
            if let Some(rl) = p.rate_limit.as_ref() {
                if let Some(onset) = rl.first_429_rps {
                    s.first_429_rps = Some(match s.first_429_rps {
                        Some(cur) => cur.min(onset),
                        None => onset,
                    });
                }
            }
            if p.r#final {
                s.finished = true;
            }
            // Forward to aggregator (drops if the receiver is gone).
            let _ = partial_tx.send((worker_id.clone(), p));
        }
    }
    s
}

/// Per-tick aggregator. Buckets per-worker Partials by tick number; when all
/// `num_workers` have reported a given tick, sums them and emits a single
/// AggregatedTick. Closes when every worker's send half drops.
async fn aggregate_loop(
    mut partial_rx: tokio::sync::mpsc::UnboundedReceiver<(String, Partial)>,
    tick_tx: Option<UnboundedSender<AggregatedTick>>,
    num_workers: usize,
) {
    let mut per_tick: HashMap<u64, HashMap<String, Partial>> = HashMap::new();
    let mut cum_completed = 0u64;
    let mut cum_pass = 0u64;
    let mut cum_fail_status = 0u64;
    while let Some((worker_id, p)) = partial_rx.recv().await {
        if p.r#final {
            continue;
        }
        let tick_n = p.tick;
        let entry = per_tick.entry(tick_n).or_default();
        entry.insert(worker_id, p);
        if entry.len() == num_workers {
            // Drain this tick's partials and aggregate.
            let bucket = per_tick.remove(&tick_n).unwrap();
            let agg = aggregate_tick(
                tick_n,
                bucket.values(),
                &mut cum_completed,
                &mut cum_pass,
                &mut cum_fail_status,
            );
            if let Some(tx) = tick_tx.as_ref() {
                let _ = tx.send(agg);
            }
        }
    }
}

fn aggregate_tick<'a, I: IntoIterator<Item = &'a Partial>>(
    tick_n: u64,
    partials: I,
    cum_completed: &mut u64,
    cum_pass: &mut u64,
    cum_fail_status: &mut u64,
) -> AggregatedTick {
    let mut this = AggThisTick {
        total: 0,
        pass: 0,
        fail_status: 0,
        fail_latency: 0,
        fail_size: 0,
        fail_content_type: 0,
        rate_limited: 0,
    };
    let mut achieved_rps_1s = 0.0_f64;
    // Minimum non-null onset across workers reporting this tick.
    let mut rate_limit_onset_rps: Option<f64> = None;
    // Sum each worker's buckets into one combined Bucket for this tick.
    // Single bucket per tick today (matches engine's emission shape).
    let mut combined: Option<AggBucket> = None;
    // HDR merge targets — same bounds as engine::hist so add() succeeds without
    // auto-resize. Workers that ship empty / malformed bytes are skipped.
    let mut merged_hist = crate::hist::new_corrected_target();
    let mut merged_uncorrected = crate::hist::new_corrected_target();
    let mut merged_any = false;
    let mut merged_uncorrected_any = false;
    for p in partials {
        if let Some(h) = crate::hist::deserialize_v2(&p.corrected_hist) {
            if merged_hist.add(&h).is_ok() {
                merged_any = true;
            }
        }
        if let Some(h) = crate::hist::deserialize_v2(&p.uncorrected_hist) {
            if merged_uncorrected.add(&h).is_ok() {
                merged_uncorrected_any = true;
            }
        }
        achieved_rps_1s += p.achieved_rps;
        if let Some(rl) = p.rate_limit.as_ref() {
            if let Some(onset) = rl.first_429_rps {
                rate_limit_onset_rps = Some(match rate_limit_onset_rps {
                    Some(cur) => cur.min(onset),
                    None => onset,
                });
            }
        }
        for b in &p.buckets {
            this.total += b.total;
            this.pass += b.pass;
            this.fail_status += b.fail_status;
            this.fail_latency += b.fail_latency;
            this.fail_size += b.fail_size;
            this.fail_content_type += b.fail_content_type;
            this.rate_limited += b.rate_limited;
            combined = Some(match combined.take() {
                None => AggBucket {
                    rps_lo: b.rps_lo,
                    rps_hi: b.rps_hi,
                    total: b.total,
                    pass: b.pass,
                    fail_status: b.fail_status,
                    fail_latency: b.fail_latency,
                    fail_size: b.fail_size,
                    fail_content_type: b.fail_content_type,
                    rate_limited: b.rate_limited,
                },
                Some(mut acc) => {
                    acc.total += b.total;
                    acc.pass += b.pass;
                    acc.fail_status += b.fail_status;
                    acc.fail_latency += b.fail_latency;
                    acc.fail_size += b.fail_size;
                    acc.fail_content_type += b.fail_content_type;
                    acc.rate_limited += b.rate_limited;
                    acc
                }
            });
        }
    }
    *cum_completed += this.total;
    *cum_pass += this.pass;
    *cum_fail_status += this.fail_status;
    // Re-bucket onto the AGGREGATE RPS axis (10 RPS wide), matching engine's
    // emission convention; the worker-local bucket edges may differ if each
    // worker's per-worker RPS slice doesn't land on a clean boundary.
    let agg_lo = (achieved_rps_1s / 10.0).floor() * 10.0;
    let buckets = vec![AggBucket {
        rps_lo: agg_lo,
        rps_hi: agg_lo + 10.0,
        total: this.total,
        pass: this.pass,
        fail_status: this.fail_status,
        fail_latency: this.fail_latency,
        fail_size: this.fail_size,
        fail_content_type: this.fail_content_type,
        rate_limited: this.rate_limited,
    }];
    // `combined` is currently unused since we re-bucket on the aggregate axis;
    // kept here as a hook for when worker-local buckets matter (mixed loads).
    let _ = combined;
    let (percentiles_so_far, merged_hist_v2) = if merged_any {
        let pct = AggPercentiles {
            p50_us: merged_hist.value_at_quantile(0.5),
            p95_us: merged_hist.value_at_quantile(0.95),
            p99_us: merged_hist.value_at_quantile(0.99),
        };
        let bytes = crate::hist::serialize_v2(&merged_hist);
        (pct, bytes)
    } else {
        (AggPercentiles::default(), Vec::new())
    };
    let merged_uncorrected_hist_v2 = if merged_uncorrected_any {
        crate::hist::serialize_v2(&merged_uncorrected)
    } else {
        Vec::new()
    };
    AggregatedTick {
        elapsed_s: tick_n,
        achieved_rps_1s,
        completed_total: *cum_completed,
        pass_total: *cum_pass,
        fail_status_total: *cum_fail_status,
        this_tick: this,
        percentiles_so_far,
        buckets,
        rate_limit_onset_rps,
        merged_hist_v2,
        merged_uncorrected_hist_v2,
    }
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
            headers: spec.target_headers.clone(),
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
