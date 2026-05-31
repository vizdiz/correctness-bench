//! `bench` — thin client of the control plane (contracts/api.md).
//!
//! Workflow:
//!   1. Build a CreateRunRequest from CLI flags.
//!   2. POST it to {control}/v1/runs → get `{run_id, status}`.
//!   3. Subscribe `/v1/runs/{run_id}/stream` (SSE) and render one dashboard
//!      line per tick (achieved rps, pass rate, p50/p99).
//!   4. Wait for the run's `duration_s` (+ grace) or Ctrl-C; print the final
//!      summary derived from the accumulated tick state.
//!   5. Exit codes:
//!      - 0  — run completed (at least one tick received in time)
//!      - 1  — engine error: aborted, or no ticks ever arrived
//!      - 2  — bad args / control plane unreachable / spec rejected

use std::process::ExitCode;
use std::time::Duration;

use anyhow::{anyhow, Context};
use clap::Parser;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::{timeout, Instant};

#[derive(Parser, Debug)]
#[command(
    name = "bench",
    about = "Thin client of the correctness-bench control plane"
)]
struct Cli {
    /// Target URL to benchmark. Sent as `target.url`.
    #[arg(short = 't', long)]
    target: String,

    /// HTTP method.
    #[arg(short = 'm', long, default_value = "GET")]
    method: String,

    /// Target requests per second.
    #[arg(short = 'R', long, default_value_t = 100.0)]
    rps: f64,

    /// Run duration in seconds.
    #[arg(short = 'd', long, default_value_t = 30)]
    duration_s: u64,

    /// Open connections (pool size).
    #[arg(short = 'c', long, default_value_t = 50)]
    connections: u32,

    /// Load model.
    #[arg(long, default_value = "open", value_parser = ["open", "closed"])]
    load_model: String,

    /// Expected HTTP status codes (repeatable; empty = any 2xx).
    #[arg(long)]
    expected_status: Vec<u16>,

    /// Optional inline max-latency-ms assertion.
    #[arg(long)]
    max_latency_ms: Option<u64>,

    /// Optional run name.
    #[arg(short = 'n', long)]
    name: Option<String>,

    /// API key sent as `Authorization: Bearer <key>` to the TARGET.
    /// Never persisted by control (verified by the credential canary test).
    #[arg(long, env = "BENCH_API_KEY", hide_env_values = true)]
    key: Option<String>,

    /// Control plane base URL.
    #[arg(long, env = "BENCH_CONTROL_URL", default_value = "http://localhost:8000")]
    control: String,

    /// Web UI base URL for the dashboard link printed on start.
    #[arg(long, env = "BENCH_WEB_URL", default_value = "http://localhost:5173")]
    web: String,

    /// Per-request timeout (ms).
    #[arg(long, default_value_t = 30_000)]
    timeout_ms: u64,
}

#[derive(Serialize)]
struct CreateRunRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    target: TargetSpec,
    target_rps: f64,
    duration_s: u64,
    load_model: String,
    connections: u32,
    keepalive: bool,
    assert: serde_json::Value,
    rate_limit_policy: serde_json::Value,
}

#[derive(Serialize)]
struct TargetSpec {
    url: String,
    method: String,
    headers: serde_json::Map<String, serde_json::Value>,
    timeout_ms: u64,
    verify_tls: bool,
}

#[derive(Deserialize)]
struct CreateRunResponse {
    run_id: String,
    #[allow(dead_code)]
    status: String,
}

#[derive(Default, Clone)]
struct TickAccum {
    last: Option<Tick>,
    received: u64,
    /// Per-tier totals — engine only ships `fail_status_total` cumulative;
    /// the other tiers we sum from `this_tick.*` on the client.
    fail_status_total: u64,
    fail_latency_total: u64,
    fail_size_total: u64,
    fail_content_type_total: u64,
}

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
struct Tick {
    elapsed_s: u64,
    achieved_rps_1s: f64,
    completed_total: u64,
    pass_total: u64,
    fail_status_total: u64,
    this_tick: ThisTick,
    percentiles_so_far: PercentilesSoFar,
    #[serde(default)]
    buckets: Vec<serde_json::Value>,
}

#[derive(Deserialize, Clone, Debug)]
struct ThisTick {
    total: u64,
    pass: u64,
    #[allow(dead_code)]
    fail_status: u64,
    #[serde(default)]
    fail_latency: u64,
    #[serde(default)]
    fail_size: u64,
    #[serde(default)]
    fail_content_type: u64,
}

#[derive(Deserialize, Clone, Debug, Default)]
struct PercentilesSoFar {
    #[serde(default)]
    p50_us: u64,
    #[serde(default)]
    p99_us: u64,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("bench: failed to start runtime: {e}");
            return ExitCode::from(2);
        }
    };
    match rt.block_on(run(cli)) {
        Ok(ExitKind::Completed) => ExitCode::SUCCESS,
        Ok(ExitKind::Aborted) | Ok(ExitKind::EngineError) => ExitCode::from(1),
        Err(BenchError::Unreachable(msg)) | Err(BenchError::BadSpec(msg)) => {
            eprintln!("bench: {msg}");
            ExitCode::from(2)
        }
        Err(BenchError::Internal(e)) => {
            eprintln!("bench: internal error: {e:#}");
            ExitCode::from(1)
        }
    }
}

enum ExitKind {
    Completed,
    Aborted,
    EngineError,
}

#[derive(Debug)]
enum BenchError {
    Unreachable(String),
    BadSpec(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for BenchError {
    fn from(e: anyhow::Error) -> Self {
        BenchError::Internal(e)
    }
}

async fn run(cli: Cli) -> Result<ExitKind, BenchError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| BenchError::Internal(anyhow!("build http client: {e}")))?;

    let create = build_create_request(&cli);
    let run_id = post_run(&client, &cli.control, &create).await?;

    println!(
        "bench: created run {run_id}\n       dashboard: {web}/runs/{run_id}\n       streaming live ticks ({d}s)...",
        web = cli.web.trim_end_matches('/'),
        d = cli.duration_s,
    );

    let stream_url = format!("{}/v1/runs/{}/stream", cli.control.trim_end_matches('/'), run_id);

    let accum = TickAccum::default();
    let accum_cell = std::sync::Arc::new(std::sync::Mutex::new(accum));

    // Grace beyond duration_s for the last tick + body drains.
    let total_wait = Duration::from_secs(cli.duration_s + 5);
    let watcher = stream_ticks(client.clone(), &stream_url, accum_cell.clone());

    tokio::select! {
        result = timeout(total_wait, watcher) => {
            // Either the SSE stream ended (cleanly or with an error after the
            // engine exited) or the duration timeout fired. In every case we
            // proceed to the summary; the "no ticks received" check below
            // distinguishes "engine never ran" from "completed."
            if let Ok(Err(e)) = result {
                let a = accum_cell.lock().unwrap();
                if a.received == 0 {
                    return Err(BenchError::Internal(anyhow!("sse: {e}")));
                }
                eprintln!("bench: sse stream ended after {} tick(s) — {e}", a.received);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            eprintln!("\nbench: interrupted, aborting run...");
            let _ = abort_run(&client, &cli.control, &run_id).await;
            return Ok(ExitKind::Aborted);
        }
    }

    let final_accum = accum_cell.lock().unwrap().clone();
    if final_accum.received == 0 {
        eprintln!(
            "bench: no ticks received in {}s — is an engine worker firing this run? \n       (start one with: --push-to {} --run-id {})",
            cli.duration_s + 5,
            cli.control.trim_end_matches('/'),
            run_id,
        );
        return Ok(ExitKind::EngineError);
    }
    print_summary(&cli, &run_id, &final_accum);
    Ok(ExitKind::Completed)
}

fn build_create_request(cli: &Cli) -> CreateRunRequest {
    let mut headers = serde_json::Map::new();
    if let Some(key) = cli.key.as_ref() {
        headers.insert(
            "Authorization".into(),
            serde_json::Value::String(format!("Bearer {key}")),
        );
    }
    let mut assert = serde_json::Map::new();
    if !cli.expected_status.is_empty() {
        assert.insert("expected_status".into(), json!(cli.expected_status));
    }
    if let Some(ms) = cli.max_latency_ms {
        assert.insert("max_latency_us".into(), json!(ms * 1000));
    }
    CreateRunRequest {
        name: cli.name.clone(),
        target: TargetSpec {
            url: cli.target.clone(),
            method: cli.method.to_ascii_uppercase(),
            headers,
            timeout_ms: cli.timeout_ms,
            verify_tls: true,
        },
        target_rps: cli.rps,
        duration_s: cli.duration_s,
        load_model: cli.load_model.clone(),
        connections: cli.connections,
        keepalive: true,
        assert: serde_json::Value::Object(assert),
        rate_limit_policy: json!({ "action": "backoff", "record_onset": true }),
    }
}

async fn post_run(
    client: &reqwest::Client,
    control: &str,
    body: &CreateRunRequest,
) -> Result<String, BenchError> {
    let url = format!("{}/v1/runs", control.trim_end_matches('/'));
    let resp = match client.post(&url).json(body).send().await {
        Ok(r) => r,
        Err(e) => {
            return Err(BenchError::Unreachable(format!(
                "control plane at {control} unreachable: {e}"
            )))
        }
    };
    let status = resp.status();
    if status == reqwest::StatusCode::CREATED {
        let parsed: CreateRunResponse = resp
            .json()
            .await
            .with_context(|| "decode CreateRunResponse")
            .map_err(BenchError::Internal)?;
        Ok(parsed.run_id)
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(BenchError::BadSpec(format!(
            "POST {url} -> {status}: {body}"
        )))
    }
}

async fn abort_run(
    client: &reqwest::Client,
    control: &str,
    run_id: &str,
) -> Result<(), BenchError> {
    let url = format!("{}/v1/runs/{}/abort", control.trim_end_matches('/'), run_id);
    let _ = client.post(&url).send().await;
    Ok(())
}

/// Read SSE events and update the accumulator. Returns Ok when the stream
/// ends; never returns normally on its own — wrap in `timeout`.
async fn stream_ticks(
    client: reqwest::Client,
    url: &str,
    accum: std::sync::Arc<std::sync::Mutex<TickAccum>>,
) -> anyhow::Result<()> {
    let resp = client
        .get(url)
        .header("Accept", "text/event-stream")
        .send()
        .await
        .with_context(|| format!("subscribe SSE {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("SSE subscribe returned {}", resp.status());
    }
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let started = Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read sse chunk")?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(idx) = buf.find("\n\n") {
            let block: String = buf.drain(..idx + 2).collect();
            if let Some(event) = parse_sse(&block) {
                handle_event(&event, &accum, started);
            }
        }
    }
    Ok(())
}

struct SseEvent {
    name: String,
    data: String,
}

fn parse_sse(block: &str) -> Option<SseEvent> {
    let mut name = String::from("message");
    let mut data = String::new();
    for line in block.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(d) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(d.trim_start_matches(' '));
        } else if let Some(e) = line.strip_prefix("event:") {
            name = e.trim_start_matches(' ').to_string();
        }
    }
    if data.is_empty() {
        None
    } else {
        Some(SseEvent { name, data })
    }
}

fn handle_event(
    event: &SseEvent,
    accum: &std::sync::Arc<std::sync::Mutex<TickAccum>>,
    _started: Instant,
) {
    if event.name != "tick" {
        return;
    }
    let tick: Tick = match serde_json::from_str(&event.data) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("bench: skipping malformed tick: {e}");
            return;
        }
    };
    let pass_rate = if tick.this_tick.total > 0 {
        (tick.this_tick.pass as f64 / tick.this_tick.total as f64) * 100.0
    } else {
        0.0
    };
    let p50_ms = tick.percentiles_so_far.p50_us as f64 / 1000.0;
    let p99_ms = tick.percentiles_so_far.p99_us as f64 / 1000.0;
    println!(
        "  t={:>3}s   rps={:>5.0}   pass={:>5.1}% ({:>5}/{:>5})   p50={:>5.1}ms   p99={:>5.1}ms",
        tick.elapsed_s,
        tick.achieved_rps_1s,
        pass_rate,
        tick.this_tick.pass,
        tick.this_tick.total,
        p50_ms,
        p99_ms,
    );
    let mut a = accum.lock().unwrap();
    a.fail_status_total = tick.fail_status_total;
    a.fail_latency_total += tick.this_tick.fail_latency;
    a.fail_size_total += tick.this_tick.fail_size;
    a.fail_content_type_total += tick.this_tick.fail_content_type;
    a.last = Some(tick);
    a.received += 1;
}

fn print_summary(cli: &Cli, run_id: &str, accum: &TickAccum) {
    let last = match &accum.last {
        Some(t) => t,
        None => return,
    };
    let cum_pass_rate = if last.completed_total > 0 {
        (last.pass_total as f64 / last.completed_total as f64) * 100.0
    } else {
        0.0
    };
    let p50_ms = last.percentiles_so_far.p50_us as f64 / 1000.0;
    let p99_ms = last.percentiles_so_far.p99_us as f64 / 1000.0;
    println!();
    println!("  run            {run_id}");
    if let Some(name) = cli.name.as_ref() {
        println!("  name           {name}");
    }
    println!("  target         {} {}", cli.method.to_ascii_uppercase(), cli.target);
    println!("  ticks received {}", accum.received);
    println!("  requests       {}", last.completed_total);
    println!(
        "  pass           {} / {} = {:.1}% (cumulative)",
        last.pass_total, last.completed_total, cum_pass_rate,
    );
    println!(
        "  fail by tier   status={} latency={} size={} content_type={}",
        accum.fail_status_total,
        accum.fail_latency_total,
        accum.fail_size_total,
        accum.fail_content_type_total,
    );
    println!("  latency        p50={p50_ms:.1}ms  p99={p99_ms:.1}ms");
    println!(
        "  dashboard      {}/runs/{run_id}",
        cli.web.trim_end_matches('/'),
    );
}
