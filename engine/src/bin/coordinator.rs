//! `coordinator` — long-lived gRPC service implementing the worker-facing
//! half of `bench.proto` (Register + Heartbeat). RunSlice + Abort are dialed
//! OUT to registered workers (those bind the worker-facing impl on each
//! worker's own port).
//!
//! No external service discovery: workers POST `Register` on startup; this
//! coordinator keeps an in-memory registry. Heartbeats handle liveness.

use std::net::SocketAddr;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "coordinator", about = "correctness-bench coordinator")]
struct Cli {
    /// gRPC listen address.
    #[arg(long, env = "COORD_ADDR", default_value = "0.0.0.0:9090")]
    addr: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    eprintln!("coordinator: contract_version={}", engine::coordinator::CONTRACT_VERSION);
    let _ = engine::coordinator::serve(cli.addr).await?;
    Ok(())
}
