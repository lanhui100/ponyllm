//! Red-phase regression tests: a request whose attempts fail with
//! `LockContention` (or a mix of `LockContention` and `UpstreamUnavailable`)
//! must surface as `AllRetriesFailed(LockContention)` or its honest transient
//! kind — NEVER be promoted to `QuotaExhausted` merely because the pool contains
//! another key in quota cooldown (`any_key_quota_cooldown()`).
//!
//! Incident / Bug:
//! When `attempt_kinds` contains `LockContention` (e.g. Antigravity refresh
//! skipped due to serialization lock held by another replica), `pure_transport`
//! checked only `UpstreamUnavailable`. Therefore `!pure_transport` evaluated to true,
//! and if any unrelated key was in quota cooldown, the terminal kind was rewritten
//! to `QuotaExhausted` → gateway answered 429 quota_exhausted, Sentry recorded
//! `error_kind: "QuotaExhausted"`, and downstream/DSH falsely reported quota exhausted!

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use ponyllm_core::error::{CoreError, GatewayErrorKind};
use ponyllm_core::executor::UpstreamExecutor;
use ponyllm_core::pool::refresh_gate::{RefreshGate, RefreshGateError, RefreshGateGuard};
use ponyllm_core::pool::*;

#[derive(Debug, Default)]
struct MockSkipGate;
impl RefreshGateGuard for MockSkipGate {}

#[async_trait::async_trait]
impl RefreshGate for MockSkipGate {
    async fn try_acquire(
        &self,
        _key_id: &str,
    ) -> Result<Option<Box<dyn RefreshGateGuard + Send + Sync>>, RefreshGateError> {
        Ok(None) // Simulate another replica holding the serialization lock
    }
}

fn build_lock_busy_key(id: &str, priority: u32) -> ApiKeyEntry {
    let cred = AntigravityCredential {
        access_token: None, // Missing token forces refresh
        refresh_token: "rf-token".to_string(),
        client_id: "client-id".to_string(),
        client_secret: "client-secret".to_string(),
        project_id: "proj-1".to_string(),
        expiry: None,
    };
    let mgr = Arc::new(AntigravityTokenManager::new(
        id,
        cred,
        reqwest::Client::new(),
    ));
    mgr.set_refresh_gate(Some(Arc::new(MockSkipGate)));
    ApiKeyEntry::new_antigravity(id, mgr, priority, 10)
}

fn quota_cooldown_error() -> PoolErrorType {
    PoolErrorType::QuotaExhausted {
        retry_after: Some(Duration::from_secs(3600)),
    }
}

fn payload() -> serde_json::Value {
    json!({
        "model": "gemini-3.8-flash",
        "messages": [{"role": "user", "content": "hello"}],
    })
}

/// Scenario: All attempts hit LockContention, but pool has an unrelated quota-cooled key.
#[tokio::test]
async fn lock_contention_with_unrelated_quota_cooldown_must_not_promote_to_quota_exhausted() {
    let pool = Arc::new(KeyPool::new("antigravity", RoutingStrategy::Priority));
    pool.add_key(build_lock_busy_key("ag-lock-key-1", 1));
    pool.add_key(build_lock_busy_key("ag-lock-key-2", 2));

    // An unrelated key k-quota is in quota cooldown.
    pool.add_key(ApiKeyEntry::new("k-quota", "token-quota", 3, 10));
    pool.record_error("k-quota", quota_cooldown_error());
    assert!(pool.any_key_quota_cooldown());

    let executor = UpstreamExecutor::new(pool.clone(), 2);
    let err = executor
        .execute_json_request("https://api.example.com/v1/chat/completions", &payload())
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            CoreError::AllRetriesFailed {
                kind: GatewayErrorKind::LockContention,
                ..
            }
        ),
        "LockContention must NOT be promoted to QuotaExhausted, got: {:?}",
        err
    );

    // Formatted message must identify lock contention, NOT quota exhaustion
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("lock busy/contention"),
        "error message must contain 'lock busy/contention', got: {}",
        err_msg
    );
    assert!(
        !err_msg.to_ascii_lowercase().contains("quota exhausted"),
        "error message must NOT claim quota exhaustion, got: {}",
        err_msg
    );
}

/// Scenario: Mixed failure (UpstreamUnavailable + LockContention) with unrelated quota key.
#[tokio::test]
async fn mixed_transport_and_lock_contention_must_not_promote_to_quota_exhausted() {
    let pool = Arc::new(KeyPool::new("antigravity", RoutingStrategy::Priority));

    // Key 1: regular key that fails with network error / invalid endpoint
    pool.add_key(ApiKeyEntry::new("k-net", "token-net", 1, 10));
    // Key 2: lock busy key
    pool.add_key(build_lock_busy_key("ag-lock-key", 2));
    // Key 3: unrelated key in quota cooldown
    pool.add_key(ApiKeyEntry::new("k-quota", "token-quota", 3, 10));
    pool.record_error("k-quota", quota_cooldown_error());
    assert!(pool.any_key_quota_cooldown());

    let executor = UpstreamExecutor::new(pool.clone(), 2);
    // Use an unroutable local address to force UpstreamUnavailable on Key 1
    let err = executor
        .execute_json_request("http://127.0.0.1:54321/v1/chat/completions", &payload())
        .await
        .unwrap_err();

    assert!(
        !matches!(
            err,
            CoreError::AllRetriesFailed {
                kind: GatewayErrorKind::QuotaExhausted,
                ..
            }
        ),
        "Mixed transport + lock contention must NOT be promoted to QuotaExhausted, got: {:?}",
        err
    );

    let err_msg = err.to_string();
    assert!(
        !err_msg.to_ascii_lowercase().contains("quota exhausted"),
        "error message must NOT claim quota exhaustion, got: {}",
        err_msg
    );
}

/// Scenario: Streaming variant of LockContention with unrelated quota key.
#[tokio::test]
async fn lock_contention_stream_must_not_promote_to_quota_exhausted() {
    let pool = Arc::new(KeyPool::new("antigravity", RoutingStrategy::Priority));
    pool.add_key(build_lock_busy_key("ag-lock-key-1", 1));
    pool.add_key(ApiKeyEntry::new("k-quota", "token-quota", 2, 10));
    pool.record_error("k-quota", quota_cooldown_error());

    let executor = UpstreamExecutor::new(pool.clone(), 1);
    let mut stream_payload = payload();
    stream_payload["stream"] = json!(true);

    let err = executor
        .execute_stream_request(
            "https://api.example.com/v1/chat/completions",
            &stream_payload,
        )
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            CoreError::AllRetriesFailed {
                kind: GatewayErrorKind::LockContention,
                ..
            }
        ),
        "Streaming LockContention must NOT be promoted to QuotaExhausted, got: {:?}",
        err
    );
}
