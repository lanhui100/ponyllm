//! Red-phase regression tests: a request whose EVERY attempt fails with a
//! TRANSPORT fault (TTFB timeout / network error) must surface as
//! `AllRetriesFailed(UpstreamUnavailable)` — never be reclassified to
//! `QuotaExhausted` merely because the pool happens to contain ANOTHER key in
//! quota cooldown (`any_key_quota_cooldown()`).
//!
//! Incident: dsh reported "当前请求的额度已用尽" while the gemini-3.8-flash
//! account had NOT exhausted quota. Root cause: executor saw every attempt die
//! as transport (`UpstreamUnavailable`), then on the follow-up `NoAvailableKey`
//! the pool had a quota-cooling key (`any_key_quota_cooldown() == true`) and
//! the terminal kind was rewritten to `QuotaExhausted` → gateway answered
//! 429 insufficient_quota/quota_exhausted → dsh's `isQuotaExceededError`
//! promoted it to QUOTA.
//!
//! Current implementation MUST FAIL these assertions (red); the fix keeps them
//! green without touching the honest-quota path (all-429 → QuotaExhausted).

use std::sync::Arc;
use std::time::Duration;

use axum::{routing::post, Json, Router};
use axum::response::IntoResponse;
use serde_json::json;

use ponyllm_core::error::{CoreError, GatewayErrorKind};
use ponyllm_core::executor::*;
use ponyllm_core::pool::*;

fn quota_cooldown_error() -> PoolErrorType {
    PoolErrorType::QuotaExhausted {
        retry_after: Some(Duration::from_secs(3600)),
    }
}

fn transport_payload() -> serde_json::Value {
    json!({
        "model": "gemini-3.8-flash",
        "messages": [{"role": "user", "content": "hello"}],
    })
}

/// Upstream that never answers within the TTFB budget: every key attempt dies
/// as a transport timeout, yet the pool also holds a pre-cooled quota key.
async fn build_transport_scenario() -> (String, Arc<KeyPool>) {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            (
                axum::http::StatusCode::OK,
                Json(json!({"choices": [{"message": {"content": "too late"}}]})),
            )
                .into_response()
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let endpoint = format!("http://{}/v1/chat/completions", addr);

    let pool = Arc::new(KeyPool::new("antigravity", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new("k1", "token-k1", 1, 10));
    pool.add_key(ApiKeyEntry::new("k2", "token-k2", 2, 10));
    pool.add_key(ApiKeyEntry::new("k3", "token-k3", 3, 10));

    // k3 is already in quota cooldown BEFORE the request: the pool has
    // `any_key_quota_cooldown() == true` even though no attempt in THIS
    // request hit a quota 429. This is the trap: transport failures must not
    // inherit the unrelated quota signal from k3.
    pool.record_error("k3", quota_cooldown_error());
    assert_eq!(pool.get_key_status("k3"), Some(KeyState::CoolingDown));
    assert!(pool.any_key_quota_cooldown(), "k3 must make the quota-cooldown visible");

    (endpoint, pool)
}

// ---------------------------------------------------------------------------
// 1. JSON (non-streaming): all-transport failure must stay UpstreamUnavailable.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn all_transport_failures_with_unrelated_quota_cooldown_stay_upstream_unavailable_json() {
    let (endpoint, pool) = build_transport_scenario().await;

    let executor = UpstreamExecutor::new(pool.clone(), 3)
        .with_ttfb_timeout(Some(Duration::from_millis(50)));
    let err = executor
        .execute_json_request(&endpoint, &transport_payload())
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            CoreError::AllRetriesFailed {
                kind: GatewayErrorKind::UpstreamUnavailable,
                ..
            }
        ),
        "all-transport failure must surface UpstreamUnavailable even with an unrelated quota-cooling key in the pool, got: {:?}",
        err
    );

    // Failure text must report the transport class, not quota.
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("timeout/network"),
        "error text must contain 'timeout/network', got: {}",
        err_msg
    );
    assert!(
        !err_msg.contains("quota"),
        "error text must NOT claim quota exhaustion, got: {}",
        err_msg
    );
}

// ---------------------------------------------------------------------------
// 2. Streaming variant: same scenario through execute_stream_request.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn all_transport_failures_with_unrelated_quota_cooldown_stay_upstream_unavailable_stream() {
    let (endpoint, pool) = build_transport_scenario().await;

    let executor = UpstreamExecutor::new(pool.clone(), 3)
        .with_ttfb_timeout(Some(Duration::from_millis(50)));
    let mut payload = transport_payload();
    payload["stream"] = json!(true);

    let err = executor
        .execute_stream_request(&endpoint, &payload)
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            CoreError::AllRetriesFailed {
                kind: GatewayErrorKind::UpstreamUnavailable,
                ..
            }
        ),
        "stream variant: all-transport failure must surface UpstreamUnavailable even with an unrelated quota-cooling key, got: {:?}",
        err
    );

    let err_msg = err.to_string();
    assert!(
        err_msg.contains("timeout/network"),
        "stream error text must contain 'timeout/network', got: {}",
        err_msg
    );
    assert!(
        !err_msg.contains("quota"),
        "stream error text must NOT claim quota exhaustion, got: {}",
        err_msg
    );
}

// ---------------------------------------------------------------------------
// 3. Network wire errors must not poison keys into cooldown, and must fail fast
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_network_connection_failures_do_not_poison_keys_to_cooldown() {
    let pool = Arc::new(KeyPool::new("antigravity", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-1", 1, 10));
    pool.add_key(ApiKeyEntry::new("k2", "sk-2", 2, 10));
    pool.add_key(ApiKeyEntry::new("k3", "sk-3", 3, 10));

    // Point to non-routable blackhole IP address (TEST-NET-1) to simulate network connect failure
    let dead_endpoint = "http://192.0.2.1:1/v1/chat/completions";
    let executor = UpstreamExecutor::new(pool.clone(), 6)
        .with_ttfb_timeout(Some(Duration::from_millis(50)));

    let mut payload = transport_payload();
    payload["stream"] = json!(true);

    let err = executor
        .execute_stream_request(dead_endpoint, &payload)
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            CoreError::AllRetriesFailed {
                kind: GatewayErrorKind::UpstreamUnavailable,
                ..
            }
        ),
        "Expected UpstreamUnavailable, got: {:?}",
        err
    );

    // Assert keys are NOT poisoned into Cooldown / Disabled state by pure network infrastructure drop
    assert_eq!(
        pool.get_key_status("k1"),
        Some(KeyState::Active),
        "k1 must stay Active despite network wire drop"
    );
    assert_eq!(
        pool.get_key_status("k2"),
        Some(KeyState::Active),
        "k2 must stay Active despite network wire drop"
    );
    assert_eq!(
        pool.get_key_status("k3"),
        Some(KeyState::Active),
        "k3 must stay Active despite network wire drop"
    );

    // Verify circuit breaker: max attempts was 6, but network circuit breaker aborted after 3 consecutive failures
    match err {
        CoreError::AllRetriesFailed { retries, attempted_keys, .. } => {
            assert_eq!(retries, 3, "circuit breaker must trip after 3 consecutive network failures instead of wasting all 6 attempts");
            assert_eq!(attempted_keys.len(), 3);
        }
        other => panic!("expected AllRetriesFailed, got: {:?}", other),
    }
}

