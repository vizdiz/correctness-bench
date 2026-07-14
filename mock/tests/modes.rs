//! Integration tests: spawn the real mock on an ephemeral port and drive each
//! mode below and above the cliff via reqwest.
//!
//! Cliff control is timing-independent: `cliff_rps=1_000_000` keeps every request
//! *below* the cliff; `cliff_rps=0` puts every request *above* it. The rolling
//! meter itself is unit-tested in lib.rs.

use std::time::Instant;

async fn spawn() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock::app()).await.unwrap();
    });
    format!("http://{addr}")
}

const BELOW: &str = "cliff_rps=1000000"; // every request stays below the cliff
const ABOVE: &str = "cliff_rps=0"; // every request is above the cliff

fn is_valid_json(s: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(s).is_ok()
}

#[tokio::test]
async fn healthz_ok() {
    let base = spawn().await;
    let r = reqwest::get(format!("{base}/healthz")).await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn healthy_is_ok_below_and_above_cliff() {
    let base = spawn().await;
    for q in [BELOW, ABOVE] {
        let r = reqwest::get(format!("{base}/api?mode=healthy&{q}"))
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "healthy should be 200 ({q})");
        let body = r.text().await.unwrap();
        assert!(is_valid_json(&body), "healthy body must be valid json ({q})");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["count"], 3);
    }
}

#[tokio::test]
async fn fast500_healthy_below_500_above() {
    let base = spawn().await;

    let below = reqwest::get(format!("{base}/api?mode=fast500&{BELOW}"))
        .await
        .unwrap();
    assert_eq!(below.status(), 200, "below cliff fast500 should be healthy 200");
    assert_eq!(below.headers()["x-mock-injected"], "false");

    let above = reqwest::get(format!("{base}/api?mode=fast500&{ABOVE}&pct=100"))
        .await
        .unwrap();
    assert_eq!(above.status(), 500, "above cliff fast500 should be 500");
    assert_eq!(above.headers()["x-mock-injected"], "true");
}

#[tokio::test]
async fn truncate_returns_invalid_json_above_cliff() {
    let base = spawn().await;

    let below = reqwest::get(format!("{base}/api?mode=truncate&{BELOW}"))
        .await
        .unwrap();
    assert_eq!(below.status(), 200);
    assert!(is_valid_json(&below.text().await.unwrap()));

    let above = reqwest::get(format!("{base}/api?mode=truncate&{ABOVE}&pct=100"))
        .await
        .unwrap();
    assert_eq!(above.status(), 200, "truncate keeps 200 (latency/status-blind)");
    let body = above.text().await.unwrap();
    assert!(!is_valid_json(&body), "truncate body must NOT parse: {body:?}");
    assert!(body.len() < mock::HEALTHY_BODY.len(), "truncated body should be shorter");
}

#[tokio::test]
async fn wrong_value_is_schema_valid_but_wrong_above_cliff() {
    let base = spawn().await;

    let below: serde_json::Value =
        reqwest::get(format!("{base}/api?mode=wrong_value&{BELOW}"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    assert_eq!(below["count"], 3, "below cliff is correct");

    let resp = reqwest::get(format!("{base}/api?mode=wrong_value&{ABOVE}&pct=100"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    // Schema-valid (parses, has the fields) but the value is wrong.
    assert_eq!(v["status"], "ok");
    assert_eq!(v["count"], -1, "wrong_value injects count:-1");
}

#[tokio::test]
async fn slow_ok_is_slow_but_correct_above_cliff() {
    let base = spawn().await;

    // Below cliff: fast and correct.
    let t0 = Instant::now();
    let below = reqwest::get(format!("{base}/api?mode=slow_ok&{BELOW}&base_latency_ms=0"))
        .await
        .unwrap();
    let below_ms = t0.elapsed().as_millis();
    assert_eq!(below.status(), 200);
    assert!(below_ms < (mock::SLOW_PENALTY_MS as u128), "below cliff should be fast, was {below_ms}ms");

    // Above cliff: still 200 + correct body, but late.
    let t1 = Instant::now();
    let above = reqwest::get(format!("{base}/api?mode=slow_ok&{ABOVE}&pct=100&base_latency_ms=0"))
        .await
        .unwrap();
    let above_ms = t1.elapsed().as_millis();
    assert_eq!(above.status(), 200, "slow_ok stays 200 (correctness passes)");
    let v: serde_json::Value = above.json().await.unwrap();
    assert_eq!(v["count"], 3, "slow_ok body is correct");
    assert!(
        above_ms >= (mock::SLOW_PENALTY_MS as u128),
        "above cliff should take >= {}ms, was {above_ms}ms",
        mock::SLOW_PENALTY_MS
    );
}

#[tokio::test]
async fn ratelimit_healthy_below_429_above() {
    let base = spawn().await;

    let below = reqwest::get(format!("{base}/api?mode=ratelimit&{BELOW}"))
        .await
        .unwrap();
    assert_eq!(below.status(), 200, "below cliff ratelimit should be healthy 200");
    assert_eq!(below.headers()["x-mock-injected"], "false");
    assert!(
        below.headers().get("retry-after").is_none(),
        "below cliff should not carry Retry-After"
    );

    let above = reqwest::get(format!("{base}/api?mode=ratelimit&{ABOVE}&pct=100"))
        .await
        .unwrap();
    assert_eq!(above.status(), 429, "above cliff ratelimit should be 429");
    assert_eq!(above.headers()["x-mock-injected"], "true");
    assert_eq!(above.headers()["retry-after"], "1");
}

#[tokio::test]
async fn pct_50_injects_half_above_cliff() {
    let base = spawn().await;
    let client = reqwest::Client::new();
    let mut failures = 0;
    // 100 sequential requests, all above cliff, pct=50 -> exactly 50 injected (500s).
    for _ in 0..100 {
        let r = client
            .get(format!("{base}/api?mode=fast500&{ABOVE}&pct=50"))
            .send()
            .await
            .unwrap();
        if r.status() == 500 {
            failures += 1;
        }
    }
    assert_eq!(failures, 50, "pct=50 over 100 sequential requests should give exactly 50 failures");
}
