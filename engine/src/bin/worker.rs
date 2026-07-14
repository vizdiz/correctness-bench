//! CLI wrapper around `engine::run`. Prints a wrk2-style summary and exits.

use std::time::Duration;

use clap::Parser;
use http::{Method, Uri};

use engine::worker::RunSpec;

#[derive(Parser, Debug)]
#[command(
    name = "engine-worker",
    about = "Single-worker, COO-correct load generator (gate #1 MVP)"
)]
struct Cli {
    /// Target URL (e.g. http://localhost:8080/api?mode=healthy&base_latency_ms=20)
    #[arg(short = 'u', long)]
    url: String,

    /// HTTP method.
    #[arg(short = 'm', long, default_value = "GET")]
    method: String,

    /// Target requests per second across all connections.
    #[arg(short = 'R', long)]
    rate: f64,

    /// Run duration in seconds.
    #[arg(short = 'd', long, default_value_t = 60)]
    duration_s: u64,

    /// Number of open connections (parallelism).
    #[arg(short = 'c', long, default_value_t = 50)]
    connections: usize,

    /// Per-request timeout, seconds.
    #[arg(long, default_value_t = 30)]
    timeout_s: u64,

    /// Disable HTTP keepalive (no-op in the hyper backend — kept for symmetry).
    #[arg(long)]
    no_keepalive: bool,

    /// Tokio worker threads: 0 = auto (multi-thread, num_cpus), 1 = current-thread.
    #[arg(long, default_value_t = 0)]
    worker_threads: usize,

    /// Inline status-tier assertion. Empty (default) means "any 2xx is pass."
    /// Repeat to allow multiple, e.g. `--expected-status 200 --expected-status 204`.
    #[arg(long)]
    expected_status: Vec<u16>,

    /// Emit a JSON-line per second on stdout (live tick stream): elapsed_s,
    /// achieved_rps_1s, completed_total, pass_total, fail_status_total.
    #[arg(long)]
    ticks: bool,

    /// Control plane base URL (e.g. http://control:8000). When set together
    /// with --run-id, each tick is POSTed to {url}/v1/_internal/runs/{id}/tick.
    #[arg(long)]
    push_to: Option<String>,

    /// Run UUID to push ticks against. Must be paired with --push-to.
    #[arg(long)]
    run_id: Option<String>,

    /// Linearly ramp the offered rate from 0 to --rate over --duration-s.
    /// One run sweeps the whole load axis — the cliff appears as a curve.
    #[arg(long)]
    ramp: bool,

    /// Inline latency-tier assertion: any corrected latency > N ms is fail_latency.
    #[arg(long)]
    max_latency_ms: Option<u64>,

    /// Inline size-tier assertion: Content-Length below N bytes is fail_size.
    #[arg(long)]
    min_body_bytes: Option<u64>,

    /// Inline size-tier assertion: Content-Length above N bytes is fail_size.
    #[arg(long)]
    max_body_bytes: Option<u64>,

    /// Inline content-type-tier assertion. Case-insensitive prefix match
    /// (e.g. `--content-type application/json` matches `; charset=utf-8`).
    #[arg(long)]
    content_type: Option<String>,

    /// Offload sampling cadence: capture every Nth response body for the
    /// control plane's offload eval pool (JSON Schema / JSON-path / regex
    /// tiers). 0 disables. Matches bench.proto AssertSpec.sample_every_n.
    #[arg(long, default_value_t = 0)]
    sample_every_n: u32,

    /// Cap on captured body length (bytes); truncated beyond. 0 = no cap.
    #[arg(long, default_value_t = 8192)]
    max_sampled_body_bytes: u32,

    /// What to do on HTTP 429 (rate limit). One of: backoff (honor Retry-After,
    /// default), continue (keep firing, just count), abort (stop the run). A 429
    /// is never counted as a correctness failure regardless of this setting.
    #[arg(long, default_value = "backoff")]
    rate_limit_action: String,

    /// Cap on Retry-After honoring for `--rate-limit-action backoff`, in ms.
    #[arg(long, default_value_t = 5000)]
    max_backoff_ms: u64,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .json()
        .with_current_span(true)
        .with_span_list(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let rt = if cli.worker_threads == 1 {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
    } else {
        let mut b = tokio::runtime::Builder::new_multi_thread();
        b.enable_all();
        if cli.worker_threads > 1 {
            b.worker_threads(cli.worker_threads);
        }
        b.build()?
    };

    rt.block_on(async move {
        let spec = RunSpec {
            url: Uri::try_from(cli.url.as_str())?,
            method: Method::from_bytes(cli.method.to_ascii_uppercase().as_bytes())?,
            target_rps: cli.rate,
            duration_s: cli.duration_s,
            connections: cli.connections,
            keepalive: !cli.no_keepalive,
            timeout: Duration::from_secs(cli.timeout_s),
            expected_status: cli.expected_status.clone(),
            max_latency_us: cli.max_latency_ms.map(|ms| ms * 1000),
            min_body_bytes: cli.min_body_bytes,
            max_body_bytes: cli.max_body_bytes,
            content_type: cli.content_type.clone(),
            ramp: cli.ramp,
            sample_every_n: cli.sample_every_n,
            max_sampled_body_bytes: cli.max_sampled_body_bytes,
            rate_limit_action: match cli.rate_limit_action.to_ascii_lowercase().as_str() {
                "continue" => engine::RlAction::Continue,
                "abort" => engine::RlAction::Abort,
                _ => engine::RlAction::Backoff,
            },
            max_backoff_ms: cli.max_backoff_ms,
            record_onset: true,
        };

        eprintln!(
            "engine-worker: {} {} @ {} rps for {}s, {} connections, threads={}",
            spec.method,
            spec.url,
            spec.target_rps,
            spec.duration_s,
            spec.connections,
            if cli.worker_threads == 0 { "auto".to_string() } else { cli.worker_threads.to_string() },
        );

        // Optional live tick channel. Wired when either --ticks (print JSON to
        // stdout) or --push-to + --run-id (POST each tick to control) is set.
        let push_url = match (cli.push_to.as_deref(), cli.run_id.as_deref()) {
            (Some(base), Some(rid)) => Some(format!(
                "{}/v1/_internal/runs/{}/tick",
                base.trim_end_matches('/'),
                rid
            )),
            _ => None,
        };
        let want_ticks = cli.ticks || push_url.is_some();
        // Accumulator for per-bucket pass rate, used by the cliff detector
        // post-run. Shared between the tick task and the main thread.
        let bucket_accum: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<u64, BucketCounts>>> =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
        let tick_tx = if want_ticks {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<engine::Tick>();
            let print_json = cli.ticks;
            let http = push_url.as_ref().map(|_| reqwest::Client::new());
            let push_url = push_url.clone();
            let bucket_accum = bucket_accum.clone();
            use tracing::Instrument;
            tokio::spawn(
                async move {
                    while let Some(tick) = rx.recv().await {
                        if print_json {
                            if let Ok(line) = serde_json::to_string(&tick) {
                                println!("{line}");
                            }
                        }
                        {
                            let mut m = bucket_accum.lock().unwrap();
                            for b in &tick.buckets {
                                // Key on rps_lo (integer) for the BTreeMap. Each
                                // tick contributes its 1-second counts.
                                let key = b.rps_lo as u64;
                                let entry = m.entry(key).or_default();
                                entry.rps_lo = b.rps_lo;
                                entry.rps_hi = b.rps_hi;
                                entry.total += b.total;
                                entry.pass += b.pass;
                                entry.rate_limited += b.rate_limited;
                            }
                        }
                        if let (Some(url), Some(client)) = (&push_url, &http) {
                            let req = client.post(url).json(&tick);
                            let _ = req.send().await;
                        }
                    }
                }
                .in_current_span(),
            );
            Some(tx)
        } else {
            None
        };

        let report = engine::run_with_ticks(spec, tick_tx).await?;

        // Compute cliff_rps from the accumulated bucket stream BEFORE we
        // drop into the local print block. Definition: lowest-RPS bucket
        // (by rps_lo) where pass_rate drops below 0.95. None when the run
        // stays healthy or never reached enough samples per bucket.
        let cliff_rps = {
            let m = bucket_accum.lock().unwrap();
            detect_cliff_rps(&m, 0.95, 30)
        };

        // Push a one-shot finalize to control so the run row gets its final
        // percentiles + totals + lifecycle status. Best-effort: a failure here
        // doesn't fail the binary (the report still prints locally).
        if let (Some(base), Some(rid)) = (cli.push_to.as_deref(), cli.run_id.as_deref()) {
            let finalize_url = format!(
                "{}/v1/_internal/runs/{}/finalize",
                base.trim_end_matches('/'),
                rid
            );
            // V2-deflate both corrected + uncorrected histograms for the JSON
            // histogram endpoint and the COO-delta on completed runs.
            let (corrected_b64, uncorrected_b64) = {
                use base64::Engine as _;
                let enc = base64::engine::general_purpose::STANDARD;
                let c = engine::hist::serialize_v2(&report.histos.corrected);
                let u = engine::hist::serialize_v2(&report.histos.uncorrected);
                (enc.encode(&c), enc.encode(&u))
            };
            let body = serde_json::json!({
                "effective_rps":  report.achieved_rps,
                "p50_us":  report.corrected.p50_us as i64,
                "p95_us":  report.corrected.p95_us as i64,
                "p99_us":  report.corrected.p99_us as i64,
                "p999_us": report.corrected.p999_us as i64,
                "total_requests": report.completed as i64,
                "total_pass":     report.pass as i64,
                "cliff_rps": cliff_rps,
                "status": "completed",
                "corrected_hist_b64": corrected_b64,
                "uncorrected_hist_b64": uncorrected_b64,
                "rate_limited": report.rate_limited as i64,
                "rate_limit_onset_rps": report.first_429_rps,
                "backoff_us_total": report.backoff_us_total as i64,
            });
            let client = reqwest::Client::new();
            let req = client.post(&finalize_url).json(&body);
            match req.send().await {
                Ok(r) if r.status().is_success() => {}
                Ok(r) => eprintln!("engine-worker: finalize returned {}", r.status()),
                Err(e) => eprintln!("engine-worker: finalize failed: {e}"),
            }
        }

        println!();
        println!(
            "  duration       {:>10.2}s",
            report.elapsed_ms as f64 / 1000.0
        );
        println!(
            "  requests       {:>10}    (achieved {:>7.1} req/s)",
            report.completed, report.achieved_rps
        );
        if report.conn_errors + report.timeouts > 0 {
            println!(
                "  errors         conn={} timeout={}",
                report.conn_errors, report.timeouts
            );
        }
        println!();
        println!("  Latency (corrected)   p50   p95    p99   p999    max");
        println!(
            "                     {:>5}  {:>5}  {:>5}  {:>5}  {:>5}",
            ms(report.corrected.p50_us),
            ms(report.corrected.p95_us),
            ms(report.corrected.p99_us),
            ms(report.corrected.p999_us),
            ms(report.corrected.max_us),
        );
        println!("  Latency (uncorrected) p50   p95    p99   p999    max");
        println!(
            "                     {:>5}  {:>5}  {:>5}  {:>5}  {:>5}",
            ms(report.uncorrected.p50_us),
            ms(report.uncorrected.p95_us),
            ms(report.uncorrected.p99_us),
            ms(report.uncorrected.p999_us),
            ms(report.uncorrected.max_us),
        );
        println!();
        println!(
            "  COO delta p99      {:+.1}ms  (corrected − uncorrected)",
            (report.corrected.p99_us as f64 - report.uncorrected.p99_us as f64) / 1000.0
        );
        println!();
        // Correctness denominator excludes rate-limited (429) responses — a 429
        // is neither a pass nor a fail, and never folded into the API's score.
        let correctness_denom = report.completed.saturating_sub(report.rate_limited);
        let pass_rate = if correctness_denom > 0 {
            report.pass as f64 / correctness_denom as f64 * 100.0
        } else {
            0.0
        };
        let expected_label = if report.spec.expected_status.is_empty() {
            "any 2xx".to_string()
        } else {
            report
                .spec
                .expected_status
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        println!(
            "  Correctness        {} / {} pass  ({:.1}%)  (expected status: {})",
            report.pass, correctness_denom, pass_rate, expected_label
        );
        println!(
            "  Fail by tier       status={}  latency={}  size={}  content_type={}",
            report.fail_status,
            report.fail_latency,
            report.fail_size,
            report.fail_content_type,
        );
        // Rate limiting is reported on its own line — OUR load artifact, kept
        // out of the correctness tally above.
        let onset_label = report
            .first_429_rps
            .map(|r| format!("{r:.0} rps"))
            .unwrap_or_else(|| "n/a".to_string());
        println!(
            "  Rate limited (429) {}  (onset {}, backoff {:.1}s total)",
            report.rate_limited,
            onset_label,
            report.backoff_us_total as f64 / 1_000_000.0,
        );
        Ok::<_, anyhow::Error>(())
    })?;

    Ok(())
}

fn ms(us: u64) -> String {
    format!("{:.1}", us as f64 / 1000.0)
}

#[derive(Default, Clone, Debug)]
struct BucketCounts {
    rps_lo: f64,
    rps_hi: f64,
    total: u64,
    pass: u64,
    /// 429s in this bucket — subtracted from `total` to form the correctness
    /// denominator so rate limiting never manufactures a false cliff.
    rate_limited: u64,
}

/// detect_cliff_rps: the lowest offered-RPS bucket where the cumulative pass
/// rate drops below `threshold` (typical 0.95), restricted to buckets with at
/// least `min_samples` requests so a single bad tick doesn't trigger a phantom
/// cliff. Returns the bucket midpoint, or None if no bucket fails.
fn detect_cliff_rps(
    buckets: &std::collections::BTreeMap<u64, BucketCounts>,
    threshold: f64,
    min_samples: u64,
) -> Option<f64> {
    for (_, b) in buckets.iter() {
        // Correctness denominator excludes 429s — they are OUR load artifact,
        // not the API degrading.
        let denom = b.total.saturating_sub(b.rate_limited);
        if denom < min_samples {
            continue;
        }
        let pass_rate = b.pass as f64 / denom as f64;
        if pass_rate < threshold {
            return Some((b.rps_lo + b.rps_hi) / 2.0);
        }
    }
    None
}
