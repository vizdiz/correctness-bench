//! `worker_node` - gRPC peer of the coordinator. Registers on startup, then
//! serves `RunSlice` and `Abort` from `bench.proto` on its own port.
//!
//! Flow:
//!   1. Dial the coordinator at `--coord` and call `Register(WorkerInfo)` with
//!      this node's gRPC address.
//!   2. Bind a tonic server on `--listen` and serve the worker-facing half of
//!      `Bench` (RunSlice runs an engine slice; Abort is a stub for now).
//!
//! Each `RunSlice(Assignment)` runs `engine::run_with_ticks` with the spec
//! mapped from the Assignment, streaming a `Partial` per tick back over the
//! gRPC response stream until the run completes (then one final Partial with
//! `final = true`).

use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use tonic::transport::Endpoint;
use uuid::Uuid;

use engine::coordinator::CONTRACT_VERSION;
use engine::proto::bench::bench_client::BenchClient;
use engine::proto::bench::WorkerInfo;
use engine::worker_node::serve;

#[derive(Parser, Debug)]
#[command(name = "worker_node", about = "correctness-bench worker node (gRPC peer of coordinator)")]
struct Cli {
    /// Coordinator gRPC URL (e.g. http://coordinator:9090).
    #[arg(long, env = "COORD_URL", default_value = "http://127.0.0.1:9090")]
    coord: String,

    /// gRPC listen address (this node's own server).
    #[arg(long, env = "WORKER_LISTEN", default_value = "0.0.0.0:9091")]
    listen: SocketAddr,

    /// Externally-routable address the coordinator should dial back on for
    /// RunSlice / Abort (usually `host:port` matching `--listen`'s port).
    #[arg(long, env = "WORKER_ADDRESS")]
    address: Option<String>,

    /// Stable worker id (uuid). Defaults to a fresh uuid per process.
    #[arg(long, env = "WORKER_ID")]
    worker_id: Option<String>,

    /// Optional: self-declared peak RPS this node can sustain (hint to the
    /// coordinator for slice sizing).
    #[arg(long, default_value_t = 0)]
    max_rps: u32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let worker_id = cli.worker_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    // If --address is unset and we're listening on 0.0.0.0 (the compose case),
    // build a routable address from $HOSTNAME so peers can dial us back.
    let address = cli.address.unwrap_or_else(|| {
        if cli.listen.ip().is_unspecified() {
            let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "127.0.0.1".to_string());
            format!("{}:{}", host, cli.listen.port())
        } else {
            cli.listen.to_string()
        }
    });

    eprintln!(
        "worker_node: worker_id={} listen={} address={} coord={}",
        worker_id, cli.listen, address, cli.coord,
    );

    // Register first, then start serving. The coordinator will dial back when
    // it wants to assign a slice, so it must be able to reach `address`.
    register_with_coordinator(&cli.coord, &worker_id, &address, cli.max_rps).await?;

    // Serve until interrupted.
    serve(cli.listen, worker_id).await
}

async fn register_with_coordinator(
    coord_url: &str,
    worker_id: &str,
    address: &str,
    max_rps: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    // Retry a handful of times with backoff: when both run under compose,
    // the coordinator may not be listening yet on first try.
    let mut attempt = 0u32;
    let max_attempts = 30;
    let mut delay = Duration::from_millis(200);
    loop {
        attempt += 1;
        match dial_and_register(coord_url, worker_id, address, max_rps).await {
            Ok(ack) => {
                if ack.accepted {
                    eprintln!("worker_node: registered with coordinator at {coord_url}");
                    return Ok(());
                } else {
                    return Err(format!(
                        "coordinator rejected registration: {}",
                        ack.reason
                    )
                    .into());
                }
            }
            Err(e) if attempt < max_attempts => {
                eprintln!("worker_node: register attempt {attempt} failed ({e}); retry in {:?}", delay);
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(5));
            }
            Err(e) => return Err(format!("register gave up after {attempt} attempts: {e}").into()),
        }
    }
}

async fn dial_and_register(
    coord_url: &str,
    worker_id: &str,
    address: &str,
    max_rps: u32,
) -> Result<engine::proto::bench::RegisterAck, Box<dyn std::error::Error>> {
    let channel = Endpoint::from_shared(coord_url.to_string())?
        .timeout(Duration::from_secs(5))
        .connect()
        .await?;
    let mut client = BenchClient::new(channel);
    let resp = client
        .register(WorkerInfo {
            worker_id: worker_id.to_string(),
            address: address.to_string(),
            contract_version: CONTRACT_VERSION.to_string(),
            max_rps,
        })
        .await?;
    Ok(resp.into_inner())
}
