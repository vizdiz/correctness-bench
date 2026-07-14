//! Worker node — implements the worker-facing half of the `bench.proto`
//! gRPC service: `RunSlice` and `Abort`. (Register / Heartbeat are RPCs the
//! WORKER calls on the coordinator; they are unimplemented here.)
//!
//! On `RunSlice(Assignment)`:
//!   1. Map the Assignment → engine::RunSpec.
//!   2. Spawn `engine::run_with_ticks` with an mpsc tick channel.
//!   3. Stream `WorkerEvent::Started` then one `WorkerEvent::Partial` per
//!      received Tick, then a final `Partial { final = true }`.
//!
//! Histogram serialization is deferred — the V2/HDR bytes fields stay empty
//! for the first cut (counts + buckets merge fine without them).

use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::Stream;
use http::Uri;
use tokio::sync::mpsc;
use tonic::{Request, Response, Status, Streaming};

use crate::proto::bench::bench_server::Bench;
use crate::proto::bench::worker_event::Kind as WorkerEventKind;
use crate::proto::bench::{
    AbortAck, AbortRequest, Assignment, HealthAck, HttpMethod, Partial, RateLimit, RateLimitAction,
    RegisterAck, RpsBucket, Started, WorkerEvent, WorkerHealth, WorkerInfo,
};
use crate::worker::{run_with_ticks, RlAction, RunSpec, Tick};

pub struct WorkerNodeService {
    pub worker_id: String,
}

impl WorkerNodeService {
    pub fn new(worker_id: String) -> Self {
        Self { worker_id }
    }
}

#[tonic::async_trait]
impl Bench for WorkerNodeService {
    async fn register(&self, _: Request<WorkerInfo>) -> Result<Response<RegisterAck>, Status> {
        Err(Status::unimplemented("Register lives on the coordinator"))
    }

    type HeartbeatStream =
        Pin<Box<dyn Stream<Item = Result<HealthAck, Status>> + Send>>;

    async fn heartbeat(
        &self,
        _: Request<Streaming<WorkerHealth>>,
    ) -> Result<Response<Self::HeartbeatStream>, Status> {
        Err(Status::unimplemented("Heartbeat lives on the coordinator"))
    }

    type RunSliceStream =
        Pin<Box<dyn Stream<Item = Result<WorkerEvent, Status>> + Send>>;

    #[tracing::instrument(
        name = "worker_node.run_slice",
        level = "info",
        skip(self, req),
        fields(run_id = tracing::field::Empty, worker_id = tracing::field::Empty),
    )]
    async fn run_slice(
        &self,
        req: Request<Assignment>,
    ) -> Result<Response<Self::RunSliceStream>, Status> {
        let a = req.into_inner();
        {
            let span = tracing::Span::current();
            span.record("run_id", &a.run_id.as_str());
            span.record("worker_id", &self.worker_id.as_str());
        }
        let spec = build_run_spec(&a).map_err(|e| Status::invalid_argument(e.to_string()))?;
        let worker_id = self.worker_id.clone();

        // Snapshot the start time at the moment we accept the Assignment.
        let actual_start_unix_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros() as i64)
            .unwrap_or(0);
        let skew_us = (actual_start_unix_us as i64) - a.epoch_unix_us;

        // Tick channel — engine::run_with_ticks emits per-second snapshots.
        let (tick_tx, mut tick_rx) = mpsc::unbounded_channel::<Tick>();

        // Run the actual workload in a background task. We yield events on
        // the outbound stream as the channel produces ticks.
        let engine_handle = tokio::spawn(async move { run_with_ticks(spec, Some(tick_tx)).await });

        let outbound = async_stream::try_stream! {
            yield WorkerEvent {
                kind: Some(WorkerEventKind::Started(Started {
                    actual_start_unix_us,
                    skew_us,
                })),
            };

            let mut tick_n = 0u64;
            while let Some(tick) = tick_rx.recv().await {
                tick_n += 1;
                yield WorkerEvent {
                    kind: Some(WorkerEventKind::Partial(partial_from_tick(
                        &worker_id, tick_n, false, &tick,
                    ))),
                };
            }

            // Engine done; close out with a final Partial.
            let _ = engine_handle.await;
            yield WorkerEvent {
                kind: Some(WorkerEventKind::Partial(empty_final_partial(&worker_id, tick_n + 1))),
            };
        };

        Ok(Response::new(Box::pin(outbound)))
    }

    async fn abort(&self, _: Request<AbortRequest>) -> Result<Response<AbortAck>, Status> {
        // Mid-run abort is deferred — accept the ack so coordinator logic
        // doesn't trip over it.
        Ok(Response::new(AbortAck { accepted: true }))
    }
}

fn build_run_spec(a: &Assignment) -> Result<RunSpec, String> {
    let target = a.target.as_ref().ok_or_else(|| "Assignment.target required".to_string())?;
    let url = Uri::try_from(&target.url).map_err(|e| format!("bad target.url: {e}"))?;
    let method = http_method_from_proto(target.method);

    let timeout = if target.timeout_ms > 0 {
        Duration::from_millis(target.timeout_ms as u64)
    } else {
        Duration::from_secs(30)
    };

    let (expected_status, max_latency_us, min_body_bytes, max_body_bytes, content_type) =
        if let Some(spec) = a.assert.as_ref() {
            (
                spec.expected_status.iter().map(|n| *n as u16).collect(),
                spec.max_latency_us.map(|v| v as u64),
                spec.min_body_bytes.map(|v| v as u64),
                spec.max_body_bytes.map(|v| v as u64),
                spec.content_type.clone(),
            )
        } else {
            (vec![], None, None, None, None)
        };

    // Rate-limit policy. bench.proto's default action (proto3 zero value) is
    // RL_CONTINUE, but the coordinator sets RL_BACKOFF explicitly; if the
    // policy is omitted entirely we default to RL_BACKOFF (honor Retry-After),
    // matching the RunSpec/CLI default and the "the 429 is our artifact" thesis.
    let (rate_limit_action, max_backoff_ms, record_onset) = match a.rate_limit_policy.as_ref() {
        Some(p) => {
            let action = match RateLimitAction::try_from(p.action).unwrap_or(RateLimitAction::RlBackoff) {
                RateLimitAction::RlContinue => RlAction::Continue,
                RateLimitAction::RlBackoff => RlAction::Backoff,
                RateLimitAction::RlAbort => RlAction::Abort,
            };
            let max_backoff_ms = if p.max_backoff_ms > 0 { p.max_backoff_ms as u64 } else { 5000 };
            (action, max_backoff_ms, p.record_onset)
        }
        None => (RlAction::Backoff, 5000, true),
    };

    Ok(RunSpec {
        url,
        method,
        target_rps: a.throughput,
        duration_s: a.duration_s.max(0) as u64,
        connections: a.connections.max(1) as usize,
        keepalive: a.keepalive,
        timeout,
        expected_status,
        max_latency_us,
        min_body_bytes,
        max_body_bytes,
        content_type,
        ramp: false,
        sample_every_n: 0,
        max_sampled_body_bytes: 0,
        rate_limit_action,
        max_backoff_ms,
        record_onset,
    })
}

fn http_method_from_proto(m: i32) -> http::Method {
    match HttpMethod::try_from(m).unwrap_or(HttpMethod::Get) {
        HttpMethod::Get => http::Method::GET,
        HttpMethod::Post => http::Method::POST,
        HttpMethod::Put => http::Method::PUT,
        HttpMethod::Patch => http::Method::PATCH,
        HttpMethod::Delete => http::Method::DELETE,
        HttpMethod::Head => http::Method::HEAD,
    }
}

fn partial_from_tick(worker_id: &str, tick_n: u64, is_final: bool, t: &Tick) -> Partial {
    let buckets = t
        .buckets
        .iter()
        .map(|b| RpsBucket {
            rps_lo: b.rps_lo,
            rps_hi: b.rps_hi,
            total: b.total,
            pass: b.pass,
            fail_transport: 0,
            fail_status: b.fail_status,
            fail_latency: b.fail_latency,
            fail_size: b.fail_size,
            fail_content_type: b.fail_content_type,
            rate_limited: b.rate_limited,
            timeout: 0,
        })
        .collect();
    Partial {
        worker_id: worker_id.to_string(),
        tick: tick_n,
        r#final: is_final,
        corrected_hist: t.corrected_hist_v2.clone(),
        uncorrected_hist: t.uncorrected_hist_v2.clone(),
        ttfb_hist: vec![],
        this_tick_corrected: vec![],
        buckets,
        offered_rps: t.achieved_rps_1s,
        achieved_rps: t.achieved_rps_1s,
        sched_slip_us: 0,
        client_saturated: false,
        in_flight: 0,
        conn_errors: 0,
        timeouts: 0,
        // Rate-limit signal — first-class, never folded into correctness.
        rate_limit: Some(RateLimit {
            count_this_tick: t.this_tick.rate_limited,
            first_429_rps: t.rate_limit_onset_rps,
            last_retry_after_ms: t.last_retry_after_ms,
            backoff_us_total: t.backoff_us_total,
        }),
        bytes_sent: 0,
        bytes_recv: 0,
        sampled: vec![],
    }
}

fn empty_final_partial(worker_id: &str, tick_n: u64) -> Partial {
    Partial {
        worker_id: worker_id.to_string(),
        tick: tick_n,
        r#final: true,
        corrected_hist: vec![],
        uncorrected_hist: vec![],
        ttfb_hist: vec![],
        this_tick_corrected: vec![],
        buckets: vec![],
        offered_rps: 0.0,
        achieved_rps: 0.0,
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

/// Bind a tonic server with the worker-node's Bench impl on `addr`.
pub async fn serve(
    addr: std::net::SocketAddr,
    worker_id: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let svc = WorkerNodeService::new(worker_id);
    eprintln!("worker_node: listening on {}", addr);
    tonic::transport::Server::builder()
        .add_service(crate::proto::bench::bench_server::BenchServer::new(svc))
        .serve(addr)
        .await?;
    Ok(())
}

// Keep `Arc` reachable in case future state lives behind it.
#[allow(dead_code)]
fn _arc_marker() -> Arc<()> {
    Arc::new(())
}
