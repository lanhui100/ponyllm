//! Red-phase regression tests for the 2026-11-12 incident: dsh rendered
//! "当前请求的额度已用尽" for `gemini-3.8-flash-high` while the antigravity pool
//! had NO account left — 3 accounts were ToS-disabled (`403 PERMISSION_DENIED`
//! "This service has been disabled in this account for violation of Terms of
//! Service") and the 4th only had its model quota closed.
//!
//! Two defects made that collapse into `429 quota_exhausted`:
//!   1. `TOS_ACCOUNT_DEATH_SIGNATURES` did not match the noun form "violation
//!      of terms of service", so the account-death 403 fell through to the
//!      unknown-403 branch (60s cooling, `UpstreamUnavailable`) instead of a
//!      permanent `PolicyViolation` isolate.
//!   2. The pool-exhaustion promotion treated any non-transport attempt kind as
//!      quota evidence, so auth deaths plus an unrelated quota-cooled key were
//!      rewritten to `QuotaExhausted`.
//!
//! These tests fail on the pre-fix implementation (kind is `QuotaExhausted`
//! and the message claims "quota exhausted") and must stay green afterwards.

use std::sync::Arc;
use std::time::Duration;

use axum::response::IntoResponse;
use axum::{routing::post, Json, Router};
use serde_json::json;

use ponyllm_core::error::{CoreError, GatewayErrorKind};
use ponyllm_core::executor::*;
use ponyllm_core::pool::*;

/// Verbatim upstream body observed on 2026-11-12 (7 antigravity accounts).
fn tos_disabled_body() -> String {
    json!({
        "error": {
            "code": 403,
            "message": "This service has been disabled in this account for violation of Terms of Service. Please submit an appeal to continue using this product.",
            "status": "PERMISSION_DENIED",
            "details": [{
                "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                "reason": "SERVICE_DISABLED",
                "domain": "cloudcode-pa.googleapis.com"
            }]
        }
    })
    .to_string()
}

fn transport_payload() -> serde_json::Value {
    json!({
        "model": "gemini-3.8-flash-high",
        "messages": [{"role": "user", "content": "hello"}],
    })
}

fn quota_cooldown_error() -> PoolErrorType {
    PoolErrorType::QuotaExhausted {
        retry_after: Some(Duration::from_secs(3600)),
    }
}

/// Upstream that answers every attempt with the ToS account-death 403, while
/// the pool ALSO carries a key pre-cooled by quota exhaustion (the trap: the
/// unrelated quota cooldown must not become this request's verdict).
async fn build_tos_scenario() -> (String, Arc<KeyPool>) {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async move {
            (
                axum::http::StatusCode::FORBIDDEN,
                Json(serde_json::from_str::<serde_json::Value>(&tos_disabled_body()).unwrap()),
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

    // k3: unrelated quota cooldown, written before this request starts.
    pool.record_error("k3", quota_cooldown_error());
    assert!(
        pool.any_key_quota_cooldown(),
        "k3 must expose the unrelated quota cooldown"
    );

    (endpoint, pool)
}

// ---------------------------------------------------------------------------
// 1. Classification: the noun-form ToS wording is an account death, not an
//    unknown 403 and definitely not quota.
// ---------------------------------------------------------------------------
#[test]
fn tos_disabled_403_classifies_as_policy_violation() {
    let (kind, pool_err) = classify_forbidden(&tos_disabled_body(), None);
    assert!(
        matches!(kind, GatewayErrorKind::AuthInvalid),
        "ToS-disabled account must classify as AuthInvalid (account death), got {:?}",
        kind
    );
    assert!(
        matches!(pool_err, PoolErrorType::PolicyViolation),
        "ToS-disabled account must permanently isolate the key, got {:?}",
        pool_err
    );
}

/// The signature list must stay narrow: a prompt-level 403 that merely mentions
/// "terms" is not an account death.
#[test]
fn non_tos_403_is_not_promoted_to_account_death() {
    let body = json!({
        "error": {
            "code": 403,
            "message": "The request was rejected by the safety filter (terms of a policy were violated by the prompt)",
            "status": "PERMISSION_DENIED"
        }
    })
    .to_string();
    let (kind, pool_err) = classify_forbidden(&body, None);
    assert!(
        !matches!(pool_err, PoolErrorType::PolicyViolation),
        "a prompt-level 403 must not permanently isolate the credential, got {:?}",
        pool_err
    );
    assert!(
        matches!(kind, GatewayErrorKind::UpstreamUnavailable),
        "unknown 403 stays a transient upstream failure, got {:?}",
        kind
    );
}

/// The signature list must stay narrow: model-level / project-level
/// disablements share the word "disabled" but have a different blast radius,
/// so they must stay on the recoverable unknown-403 path.
#[test]
fn model_level_and_project_level_403_are_not_account_death() {
    let bodies = [
        json!({"error": {
            "code": 403,
            "message": "The model gemini-3.8-flash has been disabled in this account.",
            "status": "PERMISSION_DENIED"
        }})
        .to_string(),
        json!({"error": {
            "code": 403,
            "message": "Cloud Code API has not been used in project 123 before or it is disabled.",
            "status": "PERMISSION_DENIED"
        }})
        .to_string(),
    ];
    for body in bodies {
        let (kind, pool_err) = classify_forbidden(&body, None);
        assert!(
            !matches!(pool_err, PoolErrorType::PolicyViolation),
            "non-account-level disablement must not burn the credential, got {:?} for {body}",
            pool_err
        );
        assert!(
            matches!(kind, GatewayErrorKind::UpstreamUnavailable),
            "non-account-level disablement stays a transient upstream failure, got {:?}",
            kind
        );
    }
}

/// Widening the account-death set must not turn a *minority* of dead keys into
/// a "broken pool": the quota boundary keeps protecting the backup provider
/// unless the dead keys dominate (strict majority).
#[test]
fn majority_permanently_unavailable_needs_a_strict_majority() {
    let pool = KeyPool::new("antigravity", RoutingStrategy::Priority);
    pool.add_key(ApiKeyEntry::new("k1", "t1", 1, 10));
    pool.add_key(ApiKeyEntry::new("k2", "t2", 2, 10));
    pool.add_key(ApiKeyEntry::new("k3", "t3", 3, 10));
    pool.add_key(ApiKeyEntry::new("k4", "t4", 4, 10));
    pool.record_error("k1", PoolErrorType::PolicyViolation);
    pool.record_error("k1", PoolErrorType::PolicyViolation);
    assert_eq!(
        pool.get_key_status("k1"),
        Some(KeyState::Disabled),
        "a second ToS violation must force the permanent isolate"
    );
    assert!(
        !pool.majority_permanently_unavailable(),
        "1 dead key of 4 must NOT cancel the quota boundary"
    );

    let mostly_dead = KeyPool::new("antigravity", RoutingStrategy::Priority);
    mostly_dead.add_key(ApiKeyEntry::new("k1", "t1", 1, 10));
    mostly_dead.add_key(ApiKeyEntry::new("k2", "t2", 2, 10));
    mostly_dead.add_key(ApiKeyEntry::new("k3", "t3", 3, 10));
    for id in ["k1", "k2"] {
        mostly_dead.record_error(id, PoolErrorType::PolicyViolation);
        mostly_dead.record_error(id, PoolErrorType::PolicyViolation);
    }
    assert_eq!(mostly_dead.get_key_status("k1"), Some(KeyState::Disabled));
    assert_eq!(mostly_dead.get_key_status("k2"), Some(KeyState::Disabled));
    assert!(
        mostly_dead.majority_permanently_unavailable(),
        "2 dead keys of 3 is a broken pool, not a quota boundary"
    );
}

// ---------------------------------------------------------------------------
// 2. JSON: all attempts are ToS deaths → terminal kind must be AuthInvalid,
//    never QuotaExhausted (the unrelated k3 quota cooldown is not evidence).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn all_tos_403_failures_surface_auth_invalid_not_quota_json() {
    let (endpoint, pool) = build_tos_scenario().await;

    let executor = UpstreamExecutor::new(pool.clone(), 3);
    let err = executor
        .execute_json_request(&endpoint, &transport_payload())
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            CoreError::AllRetriesFailed {
                kind: GatewayErrorKind::AuthInvalid,
                ..
            }
        ),
        "all-ToS failure must surface AuthInvalid even with an unrelated quota-cooling key, got: {:?}",
        err
    );

    let err_msg = err.to_string();
    assert!(
        err_msg.contains("auth invalid"),
        "error text must report the auth class, got: {}",
        err_msg
    );
    assert!(
        !err_msg.to_ascii_lowercase().contains("quota"),
        "error text must NOT claim quota exhaustion, got: {}",
        err_msg
    );
}

// ---------------------------------------------------------------------------
// 3. Streaming variant of the same scenario.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn all_tos_403_failures_surface_auth_invalid_not_quota_stream() {
    let (endpoint, pool) = build_tos_scenario().await;

    let executor = UpstreamExecutor::new(pool.clone(), 3);
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
                kind: GatewayErrorKind::AuthInvalid,
                ..
            }
        ),
        "stream: all-ToS failure must surface AuthInvalid, got: {:?}",
        err
    );
    assert!(
        !err.to_string().to_ascii_lowercase().contains("quota"),
        "stream error text must NOT claim quota exhaustion, got: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// 4. The dead accounts are isolated instead of coming back on the next minute
//    to burn another attempt.
//
//    The first account death permanently disables the key; the second is
//    downgraded to a 5-minute cooling by the existing mass-disable circuit
//    breaker (isolating it too would leave >50% of the pool dead at once). Both
//    outcomes keep the account OUT of rotation — what must never happen is the
//    pre-fix behaviour, where the key returned after a 60s "unknown 403" cool.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn tos_403_isolates_the_banned_accounts() {
    let (endpoint, pool) = build_tos_scenario().await;

    let executor = UpstreamExecutor::new(pool.clone(), 3);
    let _ = executor
        .execute_json_request(&endpoint, &transport_payload())
        .await;

    assert_eq!(
        pool.get_key_status("k1"),
        Some(KeyState::Disabled),
        "the first ToS account death must permanently isolate the key"
    );
    assert!(
        matches!(
            pool.get_key_status("k2"),
            Some(KeyState::Disabled | KeyState::CoolingDown)
        ),
        "the second ToS account death must at least leave the key out of rotation, got {:?}",
        pool.get_key_status("k2")
    );
    assert!(
        !pool.majority_permanently_unavailable(),
        "1 dead key of 3 is still a minority: the pool stays quota-protective (a broken pool needs \
         a strict majority of dead keys)"
    );
}
