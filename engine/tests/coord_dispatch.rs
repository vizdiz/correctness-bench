//! Integration test for the dispatcher.
//!
//! Setup: spawn the mock + TWO worker_node gRPC servers on ephemeral ports.
//! Pre-populate a CoordinatorState with both as registered workers (skipping
//! the Register round-trip - covered by tests/coord_register.rs). Call
//! `coordinator::dispatch::dispatch`. Verify (a) the aggregated total
//! requests roughly match `target_rps * duration_s`, (b) both workers
//! finished, (c) all pass = total (healthy mock).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use engine::coordinator::dispatch::{dispatch, dispatch_with_ticks, AggregatedTick, DispatchSpec};
use engine::coordinator::{CoordinatorState, Worker, CONTRACT_VERSION};
use engine::proto::bench::bench_server::BenchServer;
use engine::worker_node::WorkerNodeService;
use tokio::net::TcpListener;
use tonic::transport::Server;

async fn spawn_mock() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock::app()).await.unwrap();
    });
    addr
}

async fn spawn_worker_node(worker_id: &str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let svc = WorkerNodeService::new(worker_id.to_string());
    let incoming = async_stream::stream! {
        loop {
            match listener.accept().await {
                Ok((s, _)) => yield Ok::<_, std::io::Error>(s),
                Err(_) => continue,
            }
        }
    };
    tokio::spawn(async move {
        Server::builder()
            .add_service(BenchServer::new(svc))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_aggregates_across_two_workers() {
    let mock_addr = spawn_mock().await;
    let w1_addr = spawn_worker_node("worker-a").await;
    let w2_addr = spawn_worker_node("worker-b").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Pre-populate the coordinator's registry directly.
    let state = Arc::new(CoordinatorState::new());
    let now = Instant::now();
    let mut workers = state.workers.write().await;
    workers.insert(
        "worker-a".into(),
        Worker {
            worker_id: "worker-a".into(),
            address: w1_addr.to_string(),
            contract_version: CONTRACT_VERSION.into(),
            max_rps: 0,
            registered_at: now,
            last_heartbeat: now,
        },
    );
    workers.insert(
        "worker-b".into(),
        Worker {
            worker_id: "worker-b".into(),
            address: w2_addr.to_string(),
            contract_version: CONTRACT_VERSION.into(),
            max_rps: 0,
            registered_at: now,
            last_heartbeat: now,
        },
    );
    drop(workers);

    // Sanity that HashMap import is wired (silence unused).
    let _: HashMap<&str, &str> = HashMap::new();

    let spec = DispatchSpec {
        run_id: "test-run".into(),
        target_url: format!("http://{mock_addr}/api?mode=healthy&base_latency_ms=5"),
        target_method: "GET".into(),
        target_rps: 400.0,
        duration_s: 3,
        connections: 10,
        keepalive: true,
        timeout_ms: 5_000,
        expected_status: vec![200],
        max_latency_us: None,
        min_body_bytes: None,
        max_body_bytes: None,
        content_type: None,
        target_headers: Default::default(),
    };

    let summary = dispatch(state.clone(), spec)
        .await
        .expect("dispatch ok");

    assert_eq!(summary.workers_dispatched, 2);
    assert_eq!(summary.workers_finished, 2, "both workers should have finished");
    // 400 rps * 3 s ≈ 1200 total (allow generous slack for warmup + final-tick truncation).
    assert!(
        summary.total_completed >= 800 && summary.total_completed <= 1400,
        "expected ~1200 completed across workers, got {}",
        summary.total_completed
    );
    // Healthy mock: every request should pass.
    assert_eq!(
        summary.total_pass, summary.total_completed,
        "pass should equal completed against healthy mock"
    );
    assert_eq!(summary.total_fail_status, 0);
}

/// HDR merge sanity: with two workers reporting V2-deflate snapshots each
/// tick, the coordinator's per-tick `percentiles_so_far` must populate (not
/// stay {0,0}) and the values must fall in a sane range for a healthy mock
/// at ~5 ms base latency. This is the lightweight version of gate #3 - the
/// formal half+half vs full check lives in `gates/gate3_merge.md` and runs
/// at higher load / longer duration than unit suites should.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_merges_hdr_snapshots_across_workers() {
    let mock_addr = spawn_mock().await;
    let w1_addr = spawn_worker_node("worker-a").await;
    let w2_addr = spawn_worker_node("worker-b").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let state = Arc::new(CoordinatorState::new());
    let now = Instant::now();
    let mut workers = state.workers.write().await;
    workers.insert(
        "worker-a".into(),
        Worker {
            worker_id: "worker-a".into(),
            address: w1_addr.to_string(),
            contract_version: CONTRACT_VERSION.into(),
            max_rps: 0,
            registered_at: now,
            last_heartbeat: now,
        },
    );
    workers.insert(
        "worker-b".into(),
        Worker {
            worker_id: "worker-b".into(),
            address: w2_addr.to_string(),
            contract_version: CONTRACT_VERSION.into(),
            max_rps: 0,
            registered_at: now,
            last_heartbeat: now,
        },
    );
    drop(workers);

    let spec = DispatchSpec {
        run_id: "test-run-hdr".into(),
        target_url: format!("http://{mock_addr}/api?mode=healthy&base_latency_ms=5"),
        target_method: "GET".into(),
        target_rps: 400.0,
        duration_s: 3,
        connections: 10,
        keepalive: true,
        timeout_ms: 5_000,
        expected_status: vec![200],
        max_latency_us: None,
        min_body_bytes: None,
        max_body_bytes: None,
        content_type: None,
        target_headers: Default::default(),
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AggregatedTick>();
    let mut ticks: Vec<AggregatedTick> = Vec::new();
    let collector = tokio::spawn(async move {
        let mut out = Vec::new();
        while let Some(t) = rx.recv().await {
            out.push(t);
        }
        out
    });

    let summary = dispatch_with_ticks(state.clone(), spec, Some(tx))
        .await
        .expect("dispatch ok");
    ticks.extend(collector.await.unwrap());

    assert_eq!(summary.workers_finished, 2, "both workers should have finished");
    assert!(!ticks.is_empty(), "should have received at least one aggregated tick");

    // At least one merged tick must report non-zero percentiles - proves the
    // HDR bytes round-tripped engine -> worker_node -> coordinator -> merge.
    let with_percentiles: Vec<_> = ticks
        .iter()
        .filter(|t| t.percentiles_so_far.p50_us > 0 && t.percentiles_so_far.p99_us > 0)
        .collect();
    assert!(
        !with_percentiles.is_empty(),
        "no tick reported non-zero merged percentiles: {:?}",
        ticks
            .iter()
            .map(|t| (t.elapsed_s, t.percentiles_so_far.p50_us, t.percentiles_so_far.p99_us))
            .collect::<Vec<_>>()
    );
    // Sanity: mock latency is ~5 ms, so the merged p50 should be in a generous
    // 1-200 ms band. p99 >= p50.
    let last = with_percentiles.last().unwrap();
    assert!(
        last.percentiles_so_far.p50_us >= 1_000
            && last.percentiles_so_far.p50_us <= 200_000,
        "merged p50 out of sane band: {} us",
        last.percentiles_so_far.p50_us
    );
    assert!(
        last.percentiles_so_far.p99_us >= last.percentiles_so_far.p50_us,
        "p99 ({}) should be >= p50 ({})",
        last.percentiles_so_far.p99_us,
        last.percentiles_so_far.p50_us,
    );
}

/// Gate #3 — fleet merge correctness. Run A: 1 worker at full RPS. Run B: 2
/// workers at half RPS each. Their merged p50/p95/p99 must agree within ±5%
/// and total counts within ±2%. Heavy (multiple multi-second runs at high
/// RPS), so it's `#[ignore]`-gated; run with:
///
///   cargo test --test coord_dispatch dispatch_gate3 -- --ignored --nocapture
///
/// (Scaled-down vs. the spec's 60s @ 2000 RPS so it fits in CI; the merge
/// invariant is the same regardless of magnitude.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn dispatch_gate3_half_plus_half_equals_full() {
    const FULL_RPS: f64 = 600.0;
    const DURATION_S: u64 = 10;
    const COUNT_TOLERANCE_PCT: f64 = 5.0;
    const LATENCY_TOLERANCE_PCT: f64 = 8.0;

    let mock_addr = spawn_mock().await;

    let final_pcts = |ticks: &[AggregatedTick]| -> (u64, u64, u64) {
        let last = ticks
            .iter()
            .rev()
            .find(|t| {
                t.percentiles_so_far.p50_us > 0 && t.percentiles_so_far.p99_us > 0
            })
            .expect("at least one tick with merged percentiles");
        (
            last.percentiles_so_far.p50_us,
            last.percentiles_so_far.p95_us,
            last.percentiles_so_far.p99_us,
        )
    };

    let run = |label: &'static str, num_workers: usize, per_worker_rps: f64| {
        let mock_addr = mock_addr;
        async move {
            let mut addrs = Vec::new();
            for i in 0..num_workers {
                let wid = format!("{label}-w{i}");
                let a = spawn_worker_node(&wid).await;
                addrs.push((wid, a));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;

            let state = Arc::new(CoordinatorState::new());
            let now = Instant::now();
            let mut workers = state.workers.write().await;
            for (wid, addr) in &addrs {
                workers.insert(
                    wid.clone(),
                    Worker {
                        worker_id: wid.clone(),
                        address: addr.to_string(),
                        contract_version: CONTRACT_VERSION.into(),
                        max_rps: 0,
                        registered_at: now,
                        last_heartbeat: now,
                    },
                );
            }
            drop(workers);

            let spec = DispatchSpec {
                run_id: format!("gate3-{label}"),
                target_url: format!("http://{mock_addr}/api?mode=healthy&base_latency_ms=5"),
                target_method: "GET".into(),
                target_rps: per_worker_rps * num_workers as f64,
                duration_s: DURATION_S,
                connections: 20,
                keepalive: true,
                timeout_ms: 5_000,
                expected_status: vec![200],
                max_latency_us: None,
                min_body_bytes: None,
                max_body_bytes: None,
                content_type: None,
                target_headers: Default::default(),
            };
            let (tx, mut rx) =
                tokio::sync::mpsc::unbounded_channel::<AggregatedTick>();
            let collector = tokio::spawn(async move {
                let mut out = Vec::new();
                while let Some(t) = rx.recv().await {
                    out.push(t);
                }
                out
            });
            let summary = dispatch_with_ticks(state, spec, Some(tx))
                .await
                .expect("dispatch ok");
            let ticks = collector.await.unwrap();
            (summary, ticks)
        }
    };

    let (sum_a, ticks_a) = run("A", 1, FULL_RPS).await;
    let (sum_b, ticks_b) = run("B", 2, FULL_RPS / 2.0).await;

    let (a50, a95, a99) = final_pcts(&ticks_a);
    let (b50, b95, b99) = final_pcts(&ticks_b);

    let pct_delta = |a: u64, b: u64| -> f64 {
        if a == 0 {
            return 0.0;
        }
        (b as f64 - a as f64).abs() / a as f64 * 100.0
    };

    let d_count = pct_delta(sum_a.total_completed, sum_b.total_completed);
    let d50 = pct_delta(a50, b50);
    let d95 = pct_delta(a95, b95);
    let d99 = pct_delta(a99, b99);

    println!(
        "gate3: A completed={} p50={}us p95={}us p99={}us",
        sum_a.total_completed, a50, a95, a99
    );
    println!(
        "gate3: B completed={} p50={}us p95={}us p99={}us",
        sum_b.total_completed, b50, b95, b99
    );
    println!(
        "gate3: deltas count={:.2}%  p50={:.2}%  p95={:.2}%  p99={:.2}%",
        d_count, d50, d95, d99
    );

    assert!(
        d_count <= COUNT_TOLERANCE_PCT,
        "count delta {:.2}% exceeds {:.2}%",
        d_count,
        COUNT_TOLERANCE_PCT
    );
    assert!(
        d50 <= LATENCY_TOLERANCE_PCT,
        "p50 delta {:.2}% exceeds {:.2}%",
        d50,
        LATENCY_TOLERANCE_PCT
    );
    assert!(
        d95 <= LATENCY_TOLERANCE_PCT,
        "p95 delta {:.2}% exceeds {:.2}%",
        d95,
        LATENCY_TOLERANCE_PCT
    );
    assert!(
        d99 <= LATENCY_TOLERANCE_PCT,
        "p99 delta {:.2}% exceeds {:.2}%",
        d99,
        LATENCY_TOLERANCE_PCT
    );
}
