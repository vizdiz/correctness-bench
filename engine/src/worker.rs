//! Open-loop, COO-correct worker using **raw TCP + httparse** — wrk2's model,
//! one task per connection, sequential write/read on a single TcpStream, no
//! intermediate state-machine task, no allocation per request.
//!
//! Each connection task:
//!   - At setup: `TcpStream::connect` + `set_nodelay(true)`, pre-build the
//!     fixed request bytes ONCE.
//!   - Wait at a barrier so the schedule's epoch starts after every connection
//!     is on the wire (no first-request bias).
//!   - Each iteration: schedule (sleep then sub-ms cooperative spin), write
//!     the prebuilt request, read into a per-conn 64 KiB buffer, incrementally
//!     parse with `httparse`, advance past the full message, record latency.
//!
//! Scheduling: per-connection schedules are STAGGERED so the 100 fires per cycle
//! are spread across the cycle instead of all bursting at once. Without this,
//! the Tokio scheduler queues up 100 simultaneous wake-ups and the COO delta
//! balloons. Stagger pushed corrected p50 from ~30 ms down to ~23 ms in our
//! measurements (wrk2 is 22.6 ms).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use http::{Method, Uri};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Barrier};
use tokio::task::JoinSet;

use crate::hist::{Histos, Summary};
use crate::sched::{ConnSched, RateSchedule};

#[derive(Debug, Clone)]
pub struct RunSpec {
    pub url: Uri,
    pub method: Method,
    /// Target requests per second. With `ramp = true` this is the PEAK; the
    /// schedule linearly ramps from 0 to this value over `duration_s`.
    pub target_rps: f64,
    pub duration_s: u64,
    /// Unused in the raw-TCP backend (we always keep the same TcpStream open
    /// for the whole run). Kept for API stability.
    pub keepalive: bool,
    pub connections: usize,
    pub timeout: Duration,
    /// Inline status-tier assertion (bench.proto AssertSpec.expected_status
    /// semantics): empty = any 2xx is pass; non-empty = status must be in the
    /// list to be pass. Anything else is `fail_status`.
    pub expected_status: Vec<u16>,
    /// When true, ramp the offered rate linearly from 0 to `target_rps` over
    /// `duration_s`. Reveals the full cliff curve in a single run.
    pub ramp: bool,
}

#[derive(Debug)]
pub struct RunReport {
    pub spec: RunSpec,
    pub elapsed_ms: u128,
    pub histos: Histos,
    pub achieved_rps: f64,
    pub completed: u64,
    pub pass: u64,
    pub fail_status: u64,
    pub conn_errors: u64,
    pub timeouts: u64,
    pub corrected: Summary,
    pub uncorrected: Summary,
}

#[derive(Default)]
struct ConnResult {
    completed: u64,
    pass: u64,
    fail_status: u64,
    conn_errors: u64,
    timeouts: u64,
}

/// Classify a response per bench.proto's AssertSpec.expected_status semantics.
fn is_pass(status: u16, expected: &[u16]) -> bool {
    if expected.is_empty() {
        (200..300).contains(&status)
    } else {
        expected.contains(&status)
    }
}

/// Atomic counters shared across all connection tasks for the 1 Hz tick emitter.
/// Updated on every completed request with Relaxed ordering (we tolerate the
/// occasional out-of-order observation — exact counts are recomputed at the
/// end of the run from each task's local `ConnResult`).
#[derive(Default)]
struct LiveCounters {
    completed: AtomicU64,
    pass: AtomicU64,
    fail_status: AtomicU64,
}

/// One-second snapshot of the run's progress. Shape is a subset of api.md's
/// SSE `tick` event so the engine can grow into the canonical shape additively.
#[derive(Debug, Clone, Serialize)]
pub struct Tick {
    pub elapsed_s: u64,
    pub achieved_rps_1s: f64,
    pub completed_total: u64,
    pub pass_total: u64,
    pub fail_status_total: u64,
    /// Counts FOR THIS TICK only — what arrived in the last 1 s. Matches
    /// api.md's `this_tick` nested object. Per-tick pass rate
    /// (`this_tick.pass / this_tick.total`) is the per-second cliff.
    pub this_tick: ThisTick,
    /// Running corrected percentiles as of this tick.
    pub percentiles_so_far: PercentilesSoFar,
    /// Per-RPS correctness buckets attributable to this tick. One entry today
    /// (the bucket containing `achieved_rps_1s`); clients accumulate across
    /// ticks to plot correctness-vs-load.
    pub buckets: Vec<Bucket>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThisTick {
    pub total: u64,
    pub pass: u64,
    pub fail_status: u64,
}

/// Running-percentile snapshot. api.md SSE tick names this
/// `percentiles_so_far`. We carry corrected p50/p99 (the gate-#1 numbers).
#[derive(Debug, Clone, Serialize, Default)]
pub struct PercentilesSoFar {
    pub p50_us: u64,
    pub p99_us: u64,
}

/// One per-RPS bucket worth of correctness counts. Matches the api.md SSE
/// tick `buckets` entry shape. For now each tick carries exactly one bucket
/// (the bucket the tick's measured RPS fell into); clients aggregate across
/// many ticks to build the cliff curve.
#[derive(Debug, Clone, Serialize)]
pub struct Bucket {
    pub rps_lo: f64,
    pub rps_hi: f64,
    pub total: u64,
    pub pass: u64,
    pub fail_status: u64,
}

/// Bucket width on the offered-RPS axis. 10 RPS matches the api.md example.
const BUCKET_WIDTH_RPS: f64 = 10.0;

/// Tail-wait threshold: above this we sleep, below we spin. With staggered
/// schedules the runtime rarely has more than one task spinning at a time, so
/// a tight cooperative spin replaces the timer-wheel granularity.
const SPIN_THRESHOLD_US: u64 = 100;

/// Per-connection read buffer. 64 KiB is comfortably above any realistic
/// single HTTP/1.1 response we expect to benchmark.
const READ_BUF_BYTES: usize = 64 * 1024;

/// Drive the spec to completion with no live-tick stream. Equivalent to
/// `run_with_ticks(spec, None)`.
pub async fn run(spec: RunSpec) -> anyhow::Result<RunReport> {
    run_with_ticks(spec, None).await
}

/// Drive the spec to completion. If `tick_tx` is provided, a background
/// task emits a [`Tick`] event each second containing cumulative counters and
/// the last-second achieved RPS.
pub async fn run_with_ticks(
    spec: RunSpec,
    tick_tx: Option<mpsc::UnboundedSender<Tick>>,
) -> anyhow::Result<RunReport> {
    let host = spec
        .url
        .host()
        .context("URL missing host")?
        .to_string();
    let port = spec.url.port_u16().unwrap_or(80);
    let scheme = spec.url.scheme_str().unwrap_or("http");
    anyhow::ensure!(scheme == "http", "only http:// supported in this MVP");

    let host_header = if port == 80 {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    let path = spec
        .url
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    let per_conn_rps = spec.target_rps / spec.connections as f64;
    let per_conn_throughput_us = per_conn_rps / 1_000_000.0;
    // Per-conn schedule. For ramp, `target_rps` is the peak; intended-send
    // times follow t(n) = sqrt(2·D·n/R).
    let per_conn_schedule = if spec.ramp {
        RateSchedule::Ramp {
            duration_us: spec.duration_s.saturating_mul(1_000_000),
            peak_throughput_us: per_conn_throughput_us,
        }
    } else {
        RateSchedule::Constant { throughput_us: per_conn_throughput_us }
    };
    // Stagger uses the PEAK per-conn interval (same as target for Constant),
    // so wake-ups stay spread late in a ramp.
    let per_conn_interval_us = (1_000_000.0 / per_conn_rps) as u64;
    let stagger_us = per_conn_interval_us / spec.connections.max(1) as u64;

    // Pre-build the HTTP/1.1 request bytes once. With keepalive, the same
    // bytes are written on every iteration — zero per-request allocation.
    let req_bytes = Arc::new(format!(
        "{method} {path} HTTP/1.1\r\nHost: {host_header}\r\nAccept: */*\r\nConnection: keep-alive\r\nUser-Agent: engine-worker/0.1\r\n\r\n",
        method = spec.method.as_str(),
        path = path,
        host_header = host_header,
    ).into_bytes());

    let barrier = Arc::new(Barrier::new(spec.connections + 1));
    let (worker_start_tx, worker_start_rx_template) =
        tokio::sync::watch::channel::<Option<Instant>>(None);
    let timeout = spec.timeout;

    let expected_status = Arc::new(spec.expected_status.clone());
    let counters = Arc::new(LiveCounters::default());
    // Shared corrected/uncorrected histograms — all conn tasks lock briefly to
    // record. At 2000 RPS aggregate the per-lock cost is microseconds/sec total,
    // and centralizing lets the tick task and final report read the same source
    // of truth without a per-task merge step.
    let histos = Arc::new(Mutex::new(Histos::new()));

    let mut joinset: JoinSet<ConnResult> = JoinSet::new();
    for conn_id in 0..spec.connections {
        let host = host.clone();
        let barrier = barrier.clone();
        let req_bytes = req_bytes.clone();
        let expected_status = expected_status.clone();
        let counters = counters.clone();
        let histos = histos.clone();
        let mut rx = worker_start_rx_template.clone();
        let offset_us = conn_id as u64 * stagger_us;
        joinset.spawn(async move {
            connection_task(
                host, port, req_bytes, expected_status, counters, histos,
                per_conn_schedule, offset_us,
                spec.duration_s, timeout, barrier, &mut rx,
            )
            .await
        });
    }

    barrier.wait().await;
    let worker_start = Instant::now();
    let _ = worker_start_tx.send(Some(worker_start));

    // Spawn 1Hz tick emitter (only if the caller subscribed). The task ends
    // when the run deadline passes or when the sender is dropped.
    let tick_handle = tick_tx.map(|tx| {
        let counters = counters.clone();
        let histos = histos.clone();
        let duration_s = spec.duration_s;
        tokio::spawn(async move {
            let mut prev_completed = 0u64;
            let mut prev_pass = 0u64;
            let mut prev_fail_status = 0u64;
            for elapsed_s in 1..=duration_s {
                tokio::time::sleep_until(
                    tokio::time::Instant::from_std(worker_start + Duration::from_secs(elapsed_s)),
                )
                .await;
                let c = counters.completed.load(Ordering::Relaxed);
                let p = counters.pass.load(Ordering::Relaxed);
                let f = counters.fail_status.load(Ordering::Relaxed);
                let this_tick = ThisTick {
                    total: c.saturating_sub(prev_completed),
                    pass: p.saturating_sub(prev_pass),
                    fail_status: f.saturating_sub(prev_fail_status),
                };
                let achieved_rps_1s = this_tick.total as f64;
                prev_completed = c;
                prev_pass = p;
                prev_fail_status = f;

                // Snapshot percentiles under the lock. Brief — value_at_quantile
                // is O(buckets) but bounded by HDR's structure (~hundreds).
                let percentiles_so_far = {
                    let h = histos.lock().unwrap();
                    PercentilesSoFar {
                        p50_us: h.corrected.value_at_quantile(0.5),
                        p99_us: h.corrected.value_at_quantile(0.99),
                    }
                };

                // Bucket this tick onto the offered-RPS axis. Clients sum
                // matching-edge entries across ticks to build the cliff curve.
                let rps_lo = (achieved_rps_1s / BUCKET_WIDTH_RPS).floor() * BUCKET_WIDTH_RPS;
                let bucket = Bucket {
                    rps_lo,
                    rps_hi: rps_lo + BUCKET_WIDTH_RPS,
                    total: this_tick.total,
                    pass: this_tick.pass,
                    fail_status: this_tick.fail_status,
                };

                if tx
                    .send(Tick {
                        elapsed_s,
                        achieved_rps_1s,
                        completed_total: c,
                        pass_total: p,
                        fail_status_total: f,
                        this_tick,
                        percentiles_so_far,
                        buckets: vec![bucket],
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
    });

    let mut conn_errors = 0u64;
    let mut timeouts = 0u64;
    let mut total_completed = 0u64;
    let mut total_pass = 0u64;
    let mut total_fail_status = 0u64;
    while let Some(joined) = joinset.join_next().await {
        let res = joined.context("connection task panicked")?;
        conn_errors += res.conn_errors;
        timeouts += res.timeouts;
        total_completed += res.completed;
        total_pass += res.pass;
        total_fail_status += res.fail_status;
    }

    if let Some(h) = tick_handle {
        h.abort();
    }

    // Histos are already merged in-place (shared mutex); snapshot for the report.
    let merged = histos.lock().unwrap().clone();
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
        pass: total_pass,
        fail_status: total_fail_status,
        conn_errors,
        timeouts,
        corrected,
        uncorrected,
    })
}

#[allow(clippy::too_many_arguments)]
async fn connection_task(
    host: String,
    port: u16,
    req_bytes: Arc<Vec<u8>>,
    expected_status: Arc<Vec<u16>>,
    counters: Arc<LiveCounters>,
    histos: Arc<Mutex<Histos>>,
    schedule: RateSchedule,
    offset_us: u64,
    duration_s: u64,
    timeout: Duration,
    barrier: Arc<Barrier>,
    worker_start_rx: &mut tokio::sync::watch::Receiver<Option<Instant>>,
) -> ConnResult {
    let mut res = ConnResult::default();

    // ---- Pre-warm: TCP connect, NODELAY ----
    let mut stream = match tokio::time::timeout(
        timeout,
        TcpStream::connect((host.as_str(), port)),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(_)) | Err(_) => {
            res.conn_errors += 1;
            barrier.wait().await; // still signal so the supervisor proceeds
            return res;
        }
    };
    let _ = stream.set_nodelay(true);

    barrier.wait().await;
    let _ = worker_start_rx.changed().await;
    let worker_start = match *worker_start_rx.borrow() {
        Some(t) => t,
        None => return res,
    };
    let deadline = worker_start + Duration::from_secs(duration_s);

    let mut sched = match schedule {
        RateSchedule::Constant { throughput_us } => ConnSched::new(offset_us, throughput_us),
        RateSchedule::Ramp { duration_us, peak_throughput_us } => {
            ConnSched::ramp(offset_us, duration_us, peak_throughput_us)
        }
    };
    let mut buf = vec![0u8; READ_BUF_BYTES];
    let mut buf_len = 0usize;

    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let now_us = now.duration_since(worker_start).as_micros() as u64;
        let wait_us = sched.usec_to_next_send(now_us);
        if wait_us > 0 {
            let target = now + Duration::from_micros(wait_us);
            if wait_us > SPIN_THRESHOLD_US {
                tokio::time::sleep_until(
                    tokio::time::Instant::from_std(target - Duration::from_micros(SPIN_THRESHOLD_US)),
                )
                .await;
            }
            while Instant::now() < target {
                tokio::task::yield_now().await;
            }
        }
        if Instant::now() >= deadline {
            break;
        }

        let intended_us = sched.intended_send_time();
        let actual_send_us = worker_start.elapsed().as_micros() as u64;

        // Write the pre-built request bytes.
        if let Err(_) = stream.write_all(&req_bytes).await {
            res.conn_errors += 1;
            break;
        }

        // Read until we have a complete response (headers + body).
        let (msg_len, status) =
            match read_one_response(&mut stream, &mut buf, &mut buf_len, timeout).await {
                ReadResult::Ok { msg_len, status } => (msg_len, status),
                ReadResult::Timeout => {
                    res.timeouts += 1;
                    break;
                }
                ReadResult::Err => {
                    res.conn_errors += 1;
                    break;
                }
            };

        let recv_us = worker_start.elapsed().as_micros() as u64;
        let corrected = recv_us.saturating_sub(intended_us).max(1);
        let uncorrected = recv_us.saturating_sub(actual_send_us).max(1);
        // Brief lock — record both samples under one acquisition.
        {
            let mut h = histos.lock().unwrap();
            h.record(corrected, uncorrected);
        }
        res.completed += 1;
        counters.completed.fetch_add(1, Ordering::Relaxed);
        if is_pass(status, &expected_status) {
            res.pass += 1;
            counters.pass.fetch_add(1, Ordering::Relaxed);
        } else {
            res.fail_status += 1;
            counters.fail_status.fetch_add(1, Ordering::Relaxed);
        }

        // Shift any pipelined-extra bytes to the front. Without pipelining
        // buf_len == msg_len, so this is just a reset.
        if buf_len > msg_len {
            buf.copy_within(msg_len..buf_len, 0);
            buf_len -= msg_len;
        } else {
            buf_len = 0;
        }

        sched.advance();
    }

    res
}

enum ReadResult {
    Ok { msg_len: usize, status: u16 },
    Timeout,
    Err,
}

/// Read until exactly one complete HTTP/1.1 response is in `buf`. Returns the
/// total byte count of that response, leaving any pipelined-extra bytes after
/// that point. Content-Length only — chunked is not implemented (the mock
/// uses Content-Length). Each call is bounded by `timeout`.
async fn read_one_response(
    stream: &mut TcpStream,
    buf: &mut [u8],
    buf_len: &mut usize,
    timeout: Duration,
) -> ReadResult {
    let deadline = Instant::now() + timeout;
    loop {
        // Try to parse what we already have. The borrow on `buf` ends when the
        // match arm completes, freeing `buf` for the next read.
        let parse_outcome: ParseOutcome = {
            let mut headers = [httparse::EMPTY_HEADER; 32];
            let mut resp = httparse::Response::new(&mut headers);
            match resp.parse(&buf[..*buf_len]) {
                Ok(httparse::Status::Complete(hdr_end)) => {
                    let cl = resp
                        .headers
                        .iter()
                        .find(|h| h.name.eq_ignore_ascii_case("content-length"))
                        .and_then(|h| std::str::from_utf8(h.value).ok())
                        .and_then(|s| s.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let total = hdr_end + cl;
                    let status = resp.code.unwrap_or(0);
                    if *buf_len >= total {
                        ParseOutcome::Complete { msg_len: total, status }
                    } else {
                        ParseOutcome::NeedMore
                    }
                }
                Ok(httparse::Status::Partial) => ParseOutcome::NeedMore,
                Err(_) => ParseOutcome::Bad,
            }
        };

        match parse_outcome {
            ParseOutcome::Complete { msg_len, status } => {
                return ReadResult::Ok { msg_len, status }
            }
            ParseOutcome::Bad => return ReadResult::Err,
            ParseOutcome::NeedMore => {}
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return ReadResult::Timeout;
        }
        if *buf_len == buf.len() {
            // Response larger than our buffer.
            return ReadResult::Err;
        }
        match tokio::time::timeout(remaining, stream.read(&mut buf[*buf_len..])).await {
            Ok(Ok(0)) => return ReadResult::Err, // EOF mid-response
            Ok(Ok(n)) => *buf_len += n,
            Ok(Err(_)) => return ReadResult::Err,
            Err(_) => return ReadResult::Timeout,
        }
    }
}

enum ParseOutcome {
    Complete { msg_len: usize, status: u16 },
    NeedMore,
    Bad,
}
