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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

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
        let tick_tx = if want_ticks {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<engine::Tick>();
            let print_json = cli.ticks;
            let http = push_url.as_ref().map(|_| reqwest::Client::new());
            let push_url = push_url.clone();
            tokio::spawn(async move {
                while let Some(tick) = rx.recv().await {
                    if print_json {
                        if let Ok(line) = serde_json::to_string(&tick) {
                            println!("{line}");
                        }
                    }
                    if let (Some(url), Some(client)) = (&push_url, &http) {
                        let _ = client.post(url).json(&tick).send().await;
                    }
                }
            });
            Some(tx)
        } else {
            None
        };

        let report = engine::run_with_ticks(spec, tick_tx).await?;

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
        let pass_rate = if report.completed > 0 {
            report.pass as f64 / report.completed as f64 * 100.0
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
            report.pass, report.completed, pass_rate, expected_label
        );
        println!(
            "  Fail by tier       status={}  latency={}  size={}  content_type={}",
            report.fail_status,
            report.fail_latency,
            report.fail_size,
            report.fail_content_type,
        );
        Ok::<_, anyhow::Error>(())
    })?;

    Ok(())
}

fn ms(us: u64) -> String {
    format!("{:.1}", us as f64 / 1000.0)
}
