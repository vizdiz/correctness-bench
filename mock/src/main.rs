//! Thin binary: bind the address and serve `mock::app()`.
//! Address from `MOCK_ADDR` (default 127.0.0.1:8080). JSON-ish startup log to stdout.

use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let addr: SocketAddr = std::env::var("MOCK_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
        .parse()
        .expect("MOCK_ADDR must be host:port");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr} failed: {e}"));

    println!(
        r#"{{"service":"mock","msg":"listening","addr":"{addr}"}}"#,
        addr = listener.local_addr().unwrap()
    );

    axum::serve(listener, mock::app())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!(r#"{{"service":"mock","msg":"shutting down"}}"#);
}
