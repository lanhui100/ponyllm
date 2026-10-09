//! Family-quota gating must be *reactive* (real upstream 429), never a
//! selection-time pre-judge from probe buckets or a stale family ledger.
//!
//! Behavior contract for antigravity Gemini weekly exhaustion:
//!  1. A key whose family ledger already marks Gemini as exhausted MUST NOT be
//!     refused at selection time — when the upstream actually serves 200, the
//!     executor/pool must have attempted the request instead of answering
//!     `NoAvailableKey`.
//!  2. A real 429 (quota exhausted, retry/reset advertised) MUST record the
//!     family ledger and fail over to the next key; that key must not be
//!     scheduled again for subsequent requests.
//!  3. A terminal 429 is only acceptable when *every* upstream attempt on
//!     every schedulable key has itself returned 429.
//!
//! Note: ponyllm-core has no `wiremock` dev-dependency; like all sibling
//! integration tests in this directory we stand up a real axum mock upstream
//! on an ephemeral loopback port (tokio).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{routing::post, Json, Router};
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;

use ponyllm_core::error::CoreError;
use ponyllm_core::executor::*;
use ponyllm_core::pool::*;

fn quota_body() -> serde_json::Value {
    json!({
        "error": {
            "code": 429,
            "message": "Weekly model quota exhausted. Individual quota reached. Resets in 15h21m26s.",
            "status": "RESOURCE_EXHAUSTED",
            "details": [{
                "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                "reason": "QUOTA_EXHAUSTED",
                "domain": "cloudcode-pa.googleapis.com",
                "metadata": {"uiMessage": "true", "model": "gemini-3.8-flash-high"}
            }]
        }
    })
}

fn ok_body(content: &str) -> serde_json::Value {
    json!({
        "id": "chatcmpl-ag",
        "object": "chat.completion",
        "created": 1710000000,
        "model": "gemini-3.8-flash-high",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop"
        }]
    })
}

async fn spawn_upstream(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{}/v1/chat/completions", addr)
}

// ---------------------------------------------------------------------------
// 1. Stale/probe-derived family ledger must not refuse requests upstream
//    actually answers with 200.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn family_ledger_marking_gemini_exhausted_must_not_block_selection_when_upstream_serves_200()
{
    let calls = Arc::new(AtomicUsize::new(0));
    let cc = calls.clone();
    let endpoint = spawn_upstream(Router::new().route(
        "/v1/chat/completions",
        post(move |_headers: axum::http::HeaderMap, _body: String| {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                (
                    axum::http::StatusCode::OK,
                    Json(ok_body("upstream says yes")),
                )
                    .into_response()
            }
        }),
    ))
    .await;

    let pool = Arc::new(KeyPool::new("antigravity", RoutingStrategy::Priority));
    let key = ApiKeyEntry::new("ag-probe-marked", "ag-token-probe", 1, 10);
    // Simulate a probe/writeback verdict that Gemini weekly is exhausted.
    key.set_family_quota_exhausted(QuotaFamily::Gemini, Utc::now() + ChronoDuration::hours(6));
    pool.add_key(key);

    // The selection layer itself must not treat the stale family verdict as a
    // hard exclusion — it must let the executor discover the real state.
    let selected =
        pool.select_key_with_affinity_for_family(None, &[], None, Some(QuotaFamily::Gemini));
    assert!(
        selected.is_ok(),
        "selection-time filter must NOT pre-judge from the family ledger; upstream 200 should be reachable"
    );

    let exec = UpstreamExecutor::new(pool.clone(), 3);
    let payload = json!({
        "model": "gemini-3.8-flash-high",
        "messages": [{"role": "user", "content": "hello"}],
    });
    let resp = exec.execute_json_request(&endpoint, &payload).await;
    assert!(
        resp.is_ok(),
        "executor must attempt upstream instead of failing with NoAvailableKey, got: {:?}",
        resp.err()
    );
    let resp = resp.unwrap();
    assert_eq!(
        resp["choices"][0]["message"]["content"],
        "upstream says yes"
    );
    assert!(
        calls.load(Ordering::SeqCst) >= 1,
        "upstream must have been attempted at least once, got {}",
        calls.load(Ordering::SeqCst)
    );
}

// ---------------------------------------------------------------------------
// 2. Real upstream 429 records the family ledger, fails over, and the key is
//    not scheduled again.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn real_quota_429_records_family_ledger_and_fails_over_to_next_key() {
    let k1_calls = Arc::new(AtomicUsize::new(0));
    let k2_calls = Arc::new(AtomicUsize::new(0));
    let k1c = k1_calls.clone();
    let k2c = k2_calls.clone();
    let endpoint = spawn_upstream(Router::new().route(
        "/v1/chat/completions",
        post(move |headers: axum::http::HeaderMap, _body: String| {
            let k1c = k1c.clone();
            let k2c = k2c.clone();
            async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                if auth.contains("token-k1") {
                    k1c.fetch_add(1, Ordering::SeqCst);
                    (
                        axum::http::StatusCode::TOO_MANY_REQUESTS,
                        Json(quota_body()),
                    )
                        .into_response()
                } else {
                    k2c.fetch_add(1, Ordering::SeqCst);
                    (
                        axum::http::StatusCode::OK,
                        Json(ok_body("hello from ag-k2")),
                    )
                        .into_response()
                }
            }
        }),
    ))
    .await;

    let make_mgr = |id: &str, token: &str| {
        let cred = AntigravityCredential {
            access_token: Some(token.to_string()),
            refresh_token: format!("rf-{}", id),
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
            project_id: "proj-1".to_string(),
            expiry: Some(Utc::now() + ChronoDuration::hours(1)),
        };
        Arc::new(AntigravityTokenManager::new(
            id,
            cred,
            reqwest::Client::new(),
        ))
    };

    let pool = Arc::new(KeyPool::new("antigravity", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new_antigravity(
        "ag-k1",
        make_mgr("ag-k1", "token-k1"),
        1,
        10,
    ));
    pool.add_key(ApiKeyEntry::new_antigravity(
        "ag-k2",
        make_mgr("ag-k2", "token-k2"),
        2,
        10,
    ));

    let exec = UpstreamExecutor::new(pool.clone(), 3);
    let payload = json!({
        "model": "gemini-3.8-flash-high",
        "messages": [{"role": "user", "content": "hello"}],
    });

    let (resp, winner) = exec
        .execute_json_request_with_key(&endpoint, &payload)
        .await
        .expect("quota 429 on k1 must fail over to k2");
    assert_eq!(winner, "ag-k2", "winning key must be ag-k2");
    assert_eq!(resp["choices"][0]["message"]["content"], "hello from ag-k2");
    assert_eq!(
        k1_calls.load(Ordering::SeqCst),
        1,
        "k1 is hit exactly once; quota closes the window in-request"
    );
    assert_eq!(k2_calls.load(Ordering::SeqCst), 1);

    let snapshot = pool.snapshot_keys();
    let e1 = snapshot
        .iter()
        .find(|k| k.id == "ag-k1")
        .expect("k1 in snapshot");
    assert!(
        e1.quota_group_exhausted_for(Some(QuotaFamily::Gemini), Utc::now()),
        "k1's family ledger must record Gemini exhaustion after real 429"
    );
    let ledger = e1.quota_group_exhaustions();
    let reset = ledger
        .get("Gemini Models")
        .expect("ledger must carry the 'Gemini Models' canonical entry");
    assert!(
        *reset > Utc::now() + ChronoDuration::hours(14),
        "advertised reset (~15h21m) must be preserved, got {:?}",
        reset
    );
    let e2 = snapshot
        .iter()
        .find(|k| k.id == "ag-k2")
        .expect("k2 in snapshot");
    assert!(
        !e2.quota_group_exhausted_for(Some(QuotaFamily::Gemini), Utc::now()),
        "k2 must NOT inherit k1's exhausted verdict"
    );
    assert_eq!(
        pool.get_key_status("ag-k1"),
        Some(KeyState::CoolingDown),
        "k1 must be cooling after authentic quota 429"
    );

    // Second request: k1 must never be scheduled again.
    let k1_calls_before = k1_calls.load(Ordering::SeqCst);
    let (_r2, w2) = exec
        .execute_json_request_with_key(&endpoint, &payload)
        .await
        .expect("second request must succeed via k2");
    assert_eq!(w2, "ag-k2", "second request must go straight to k2");
    assert_eq!(
        k1_calls.load(Ordering::SeqCst),
        k1_calls_before,
        "k1 upstream must not be hit again for a Gemini request"
    );
}

// ---------------------------------------------------------------------------
// 3. Only when every upstream attempt itself returns 429 is a terminal 429
//    acceptable; every key's family ledger must have been recorded.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn all_keys_upstream_quota_429_yields_final_429_after_recording_all_family_ledgers() {
    let k1_calls = Arc::new(AtomicUsize::new(0));
    let k2_calls = Arc::new(AtomicUsize::new(0));
    let k1c = k1_calls.clone();
    let k2c = k2_calls.clone();
    let endpoint = spawn_upstream(Router::new().route(
        "/v1/chat/completions",
        post(move |headers: axum::http::HeaderMap, _body: String| {
            let k1c = k1c.clone();
            let k2c = k2c.clone();
            async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                if auth.contains("token-k1") {
                    k1c.fetch_add(1, Ordering::SeqCst);
                } else {
                    k2c.fetch_add(1, Ordering::SeqCst);
                }
                (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    Json(quota_body()),
                )
                    .into_response()
            }
        }),
    ))
    .await;

    let make_mgr = |id: &str, token: &str| {
        let cred = AntigravityCredential {
            access_token: Some(token.to_string()),
            refresh_token: format!("rf-{}", id),
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
            project_id: "proj-1".to_string(),
            expiry: Some(Utc::now() + ChronoDuration::hours(1)),
        };
        Arc::new(AntigravityTokenManager::new(
            id,
            cred,
            reqwest::Client::new(),
        ))
    };

    let pool = Arc::new(KeyPool::new("antigravity", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new_antigravity(
        "ag-k1",
        make_mgr("ag-k1", "token-k1"),
        1,
        10,
    ));
    pool.add_key(ApiKeyEntry::new_antigravity(
        "ag-k2",
        make_mgr("ag-k2", "token-k2"),
        2,
        10,
    ));

    let exec = UpstreamExecutor::new(pool.clone(), 3);
    let payload = json!({
        "model": "gemini-3.8-flash-high",
        "messages": [{"role": "user", "content": "hello"}],
    });

    let err = exec
        .execute_json_request(&endpoint, &payload)
        .await
        .expect_err("all-429 pool must surface terminal error");
    assert!(
        matches!(
            err,
            CoreError::AllRetriesFailed {
                kind: ponyllm_core::error::GatewayErrorKind::QuotaExhausted,
                ..
            }
        ),
        "expected AllRetriesFailed(QuotaExhausted), got: {:?}",
        err
    );
    // Each key attempted exactly once: no key is retried in-request after a
    // real quota 429.
    assert_eq!(k1_calls.load(Ordering::SeqCst), 1, "k1 attempted once");
    assert_eq!(k2_calls.load(Ordering::SeqCst), 1, "k2 attempted once");

    for id in ["ag-k1", "ag-k2"] {
        let entry = pool
            .snapshot_keys()
            .into_iter()
            .find(|k| k.id == id)
            .expect("key in snapshot");
        assert!(
            entry.quota_group_exhausted_for(Some(QuotaFamily::Gemini), Utc::now()),
            "{} must carry the Gemini family exhausted verdict in its ledger",
            id
        );
    }
    assert_eq!(
        pool.get_key_status("ag-k1"),
        Some(KeyState::CoolingDown),
        "k1 cooling after real 429"
    );
    assert_eq!(
        pool.get_key_status("ag-k2"),
        Some(KeyState::CoolingDown),
        "k2 cooling after real 429"
    );
}
