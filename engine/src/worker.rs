//! Open-loop, COO-correct worker. Spawns N connection tasks that fire requests
//! against a single HTTP target on a shared schedule. Each task owns its own
//! [`ConnSched`] and [`Histos`]; we merge at the end. No coordinator, no gRPC —
//! single-binary gate-#1 MVP.
//!
//! Each connection task runs the same loop:
//!   1. Compute intended send time from the original schedule.
//!   2. Sleep until then (or fire immediately if behind; catch-up = 2× per wrk2).
//!   3. Issue the request, await the response (drain the body).
//!   4. Record corrected latency (recv − intended) and uncorrected (recv − actual).
//!   5. Advance `complete`; exit when the run deadline passes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use reqwest::{Client, Method, Request, Url};
use tokio::task::JoinSet;

use crate::hist::{Histos, Summary};
use crate::sched::ConnSched;

#[derive(Debug, Clone)]
pub struct RunSpec {
    pub url: Url,
    pub method: Method,
    pub target_rps: f64,
    pub duration_s: u64,
    pub connections: usize,
    pub keepalive: bool,
    pub timeout: Duration,
}

#[derive(Debug)]
pub struct RunReport {
    pub spec: RunSpec,
    pub elapsed_ms: u128,
    pub histos: Histos,
    pub achieved_rps: f64,
    pub completed: u64,
    pub conn_errors: u64,
    pub timeouts: u64,
    pub corrected: Summary,
    pub uncorrected: Summary,
}

#[derive(Default)]
struct ConnResult {
    histos: Histos,
    completed: u64,
    conn_errors: u64,
    timeouts: u64,
}

/// Drive the spec to completion. Returns merged histos + summary.
pub async fn run(spec: RunSpec) -> anyhow::Result<RunReport> {
    let client = build_client(&spec)?;
    let request_template = Arc::new(
        client
            .request(spec.method.clone(), spec.url.clone())
            .build()
            .context("build request template")?,
    );

    // Each connection gets target_rps / connections of the schedule, so the
    // aggregate matches the spec.
    let per_conn_throughput_us =
        (spec.target_rps / spec.connections as f64) / 1_000_000.0;
    let worker_start = Instant::now();
    let deadline = worker_start + Duration::from_secs(spec.duration_s);

    let mut joinset: JoinSet<ConnResult> = JoinSet::new();
    for _ in 0..spec.connections {
        joinset.spawn(connection_loop(
            client.clone(),
            request_template.clone(),
            worker_start,
            per_conn_throughput_us,
            deadline,
        ));
    }

    let mut merged = Histos::new();
    let mut conn_errors = 0u64;
    let mut timeouts = 0u64;
    let mut total_completed = 0u64;
    while let Some(joined) = joinset.join_next().await {
        let res = joined.context("connection task panicked")?;
        merged.merge(&res.histos);
        conn_errors += res.conn_errors;
        timeouts += res.timeouts;
        total_completed += res.completed;
    }

    let elapsed = worker_start.elapsed();
    let achieved_rps = total_completed as f64 / elapsed.as_secs_f64();
    let corrected = Summary::from(&merged.corrected);
    let uncorrected = Summary::from(&merged.uncorrected);

    Ok(RunReport {
        spec,
        elapsed_ms: elapsed.as_millis(),
        histos: merged,
        achieved_rps,
        completed: total_completed,
        conn_errors,
        timeouts,
        corrected,
        uncorrected,
    })
}

async fn connection_loop(
    client: Client,
    req: Arc<Request>,
    worker_start: Instant,
    throughput_us: f64,
    deadline: Instant,
) -> ConnResult {
    let mut sched = ConnSched::new(0, throughput_us);
    let mut res = ConnResult::default();

    loop {
        let now_us = worker_start.elapsed().as_micros() as u64;
        let wait_us = sched.usec_to_next_send(now_us);
        if wait_us > 0 {
            tokio::time::sleep(Duration::from_micros(wait_us)).await;
        }
        if Instant::now() >= deadline {
            break;
        }

        let intended_us = sched.intended_send_time();
        let actual_send_us = worker_start.elapsed().as_micros() as u64;

        match client.execute(clone_request(&req)).await {
            Ok(resp) => {
                // Drain the body — a request isn't done until the response is
                // fully received (matches wrk2 semantics).
                if resp.bytes().await.is_err() {
                    res.conn_errors += 1;
                    sched.advance();
                    continue;
                }
                let recv_us = worker_start.elapsed().as_micros() as u64;
                let corrected = recv_us.saturating_sub(intended_us).max(1);
                let uncorrected = recv_us.saturating_sub(actual_send_us).max(1);
                res.histos.record(corrected, uncorrected);
                res.completed += 1;
            }
            Err(e) => {
                if e.is_timeout() {
                    res.timeouts += 1;
                } else {
                    res.conn_errors += 1;
                }
            }
        }
        sched.advance();
    }
    res
}

fn build_client(spec: &RunSpec) -> anyhow::Result<Client> {
    let builder = Client::builder()
        .http1_only() // Match wrk2 (no h2 multiplexing).
        .pool_max_idle_per_host(spec.connections * 2)
        .timeout(spec.timeout);
    let builder = if spec.keepalive {
        builder.pool_idle_timeout(Some(Duration::from_secs(90)))
    } else {
        builder.pool_idle_timeout(Some(Duration::from_secs(0)))
    };
    Ok(builder.build()?)
}

/// reqwest::Request is not Clone. For gate #1 (GET, no body) this is just
/// method + url + headers.
fn clone_request(r: &Request) -> Request {
    let mut clone = Request::new(r.method().clone(), r.url().clone());
    *clone.headers_mut() = r.headers().clone();
    clone
}
