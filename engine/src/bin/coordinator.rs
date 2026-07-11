//! `coordinator` - runs both:
//!   - bench.proto gRPC server (Register / Heartbeat for worker registration
//!     and liveness; RunSlice / Abort are unimplemented here - those are
//!     dialed OUT to registered workers).
//!   - admin HTTP server (POST /admin/runs to dispatch a run, GET
//!     /admin/workers to list, GET /healthz for compose).

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;

use engine::coordinator::{admin, CoordinatorService, CoordinatorState};
use engine::proto::bench::bench_server::BenchServer;

#[derive(Parser, Debug)]
#[command(name = "coordinator", about = "correctness-bench coordinator")]
struct Cli {
    /// gRPC listen address (worker-facing).
    #[arg(long, env = "COORD_GRPC_ADDR", default_value = "0.0.0.0:9090")]
    grpc_addr: SocketAddr,

    /// Admin HTTP listen address (control-plane / CLI facing).
    #[arg(long, env = "COORD_ADMIN_ADDR", default_value = "0.0.0.0:9091")]
    admin_addr: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    eprintln!(
        "coordinator: grpc={} admin={} contract_version={}",
        cli.grpc_addr,
        cli.admin_addr,
        engine::coordinator::CONTRACT_VERSION,
    );

    let state = Arc::new(CoordinatorState::new());

    // Spawn the gRPC server in the background.
    let grpc_state = state.clone();
    let grpc_addr = cli.grpc_addr;
    let grpc_handle = tokio::spawn(async move {
        let svc = CoordinatorService::new(grpc_state);
        tonic::transport::Server::builder()
            .add_service(BenchServer::new(svc))
            .serve(grpc_addr)
            .await
    });

    // Spawn the admin HTTP server in the background.
    let admin_state = state.clone();
    let admin_addr = cli.admin_addr;
    let admin_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(admin_addr).await?;
        axum::serve(listener, admin::router(admin_state))
            .await
            .map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(e))
    });

    // Whichever exits first stops the process.
    tokio::select! {
        res = grpc_handle => {
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => eprintln!("coordinator: grpc server error: {e}"),
                Err(e) => eprintln!("coordinator: grpc task panicked: {e}"),
            }
        }
        res = admin_handle => {
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => eprintln!("coordinator: admin server error: {e}"),
                Err(e) => eprintln!("coordinator: admin task panicked: {e}"),
            }
        }
    }
    Ok(())
}
