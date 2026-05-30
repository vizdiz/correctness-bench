//! CLI wrapper around `engine::run`. Prints a wrk2-style summary and exits.

use std::time::Duration;

use clap::Parser;
use reqwest::{Method, Url};

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

    /// Disable HTTP keepalive (default: on).
    #[arg(long)]
    no_keepalive: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let spec = RunSpec {
        url: Url::parse(&cli.url)?,
        method: Method::from_bytes(cli.method.to_ascii_uppercase().as_bytes())?,
        target_rps: cli.rate,
        duration_s: cli.duration_s,
        connections: cli.connections,
        keepalive: !cli.no_keepalive,
        timeout: Duration::from_secs(cli.timeout_s),
    };

    eprintln!(
        "engine-worker: {} {} @ {} rps for {}s, {} connections, keepalive={}",
        spec.method, spec.url, spec.target_rps, spec.duration_s, spec.connections, spec.keepalive
    );

    let report = engine::run(spec).await?;

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
    Ok(())
}

fn ms(us: u64) -> String {
    format!("{:.1}", us as f64 / 1000.0)
}
