//! Wave-3 C7 L2-AT adversarial destruction tests — opencode-zen free-quota
//! 429 fast-fail (implemented per contract
//! `.dev-team/contracts/wave-3-zen-free-fastfail.md`, frozen by Lead).
//!
//! Role: L2-AT (wave-3-zen-fastfail). Green-phase adversarial strain on the
//! IMPLEMENTED executor behavior (upstream.rs JSON path 429 branch
//! L2130-2160: `is_quota && is_zen_free_usage_limit_body && egress_pool.is_none()`
//! → fast-fail after the single attempt, `retries=1`, `attempted_keys.len()==1`,
//! ONE upstream call).
//!
//! Scope (this file = the ONLY write domain):
//!   1. Oversized / malformed 429 body boundedness — a >64 KiB hostile body
//!      with the `FreeUsageLimitError` marker deeply embedded (inside the
//!      executor's 64 KiB retention window) and a junk trailer beyond the cap.
//!      Must NOT panic, must keep the 64 KiB truncation effective, must STILL
//!      classify QuotaExhausted and fast-fail (JSON path upstream calls == 1).
//!   2. Case / wording variants — `"freeusagelimiterror"` (all-lowercase) and
//!      `"Free Usage Limit"` (spaced, mixed case) must both hit
//!      `is_zen_free_usage_limit_body` → fast-fail (calls == 1). No dependence
//!      on CamelCase spelling or a JSON `error.type` field.
//!   3. Deterministic concurrency, no amplification — 4 identical JSON requests
//!      issued simultaneously (tokio::join!) against ONE shared 3-key pool with
//!      NO egress pool, behind a server-side rendezvous barrier so all 4
//!      upstream calls are in flight before the first response. Contract: total
//!      upstream calls == 4 (one per request), no panic, no cross-key retry
//!      storm (no amplification).
//!
//! Infrastructure mirrors `failover_tests.rs` / `zen_free_quota_fastfail_tests.rs`:
//! tokio TcpListener bound to 127.0.0.1:0 + axum Router/serve +
//! `UpstreamExecutor::new(Arc<KeyPool>, 3)`. Every case compiles and runs for
//! real; no `#[ignore]`, no empty bodies, no swallowed errors (zero-fake-test
//! law). Nothing outside this file and the machine receipt is modified.

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{routing::post, Router};
use ponyllm_core::error::CoreError;
use ponyllm_core::executor::*;
use ponyllm_core::pool::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The observed opencode-zen Console 429 (`FreeUsageLimitError`) body sample
/// pinned by upstream.rs `zen_free_usage_limit_429_is_quota_not_rate_limit`
/// (L3215) — the canonical contract C7 trigger body.
const ZEN_FREE_USAGE_LIMIT_429_BODY: &str = r#"{"type":"error","error":{"type":"FreeUsageLimitError","message":"Rate limit exceeded. Please try again later."},"metadata":{}}"#;

/// The executor's error-body retention cap (upstream.rs
/// `MAX_UPSTREAM_ERROR_BYTES = 64 * 1024`): hostile upstreams are truncated to
/// this many bytes before classification / diagnostics. All byte-arithmetic in
/// this file is ASCII 1:1 (no UTF-8 multi-byte surprises).
const MAX_UPSTREAM_ERROR_BYTES: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Mock infrastructure (failover_tests.rs pattern, raw-body variant)
// ---------------------------------------------------------------------------

type MockHandler = Arc<dyn Fn(usize, &str) -> (StatusCode, Vec<u8>) + Send + Sync>;

/// Spawn a local axum mock on an ephemeral port. Every request is routed to
/// `handler(call_index, authorization_header)`; the handler returns raw body
/// bytes (so the oversized-body case can emit >64 KiB of arbitrary text).
async fn spawn_mock(handler: MockHandler) -> (String, String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: axum::http::HeaderMap, _body: String| {
            let c = c.clone();
            let handler = handler.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                let (status, body) = handler(n, &auth);
                (status, Body::from(body)).into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");
    let endpoint = format!("{base}/v1/chat/completions");
    (endpoint, base, calls)
}

/// Server-side rendezvous mock: holds every response until exactly `parties`
/// upstream calls are in flight, then answers each with the zen 429. This is
/// the deterministic-concurrency primitive (no sleeps / no flaky timing): the
/// Nth arrival releases the barrier, guaranteeing all requests POSTed before
/// any error can be recorded and cool a key. A timeout keeps a pathological
/// mismatched-executor (amplification / premature stop) from deadlocking the
/// suite — the handler then answers 500 so the counter keeps counting.
async fn spawn_rendezvous_zen_429(parties: usize) -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let barrier = Arc::new(tokio::sync::Barrier::new(parties));
    let zen_bytes = serde_json::to_vec(&zen_429_value()).expect("zen 429 body serializes");
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |_headers: axum::http::HeaderMap, _body: String| {
            let c = c.clone();
            let barrier = barrier.clone();
            let zen_bytes = zen_bytes.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                let waited =
                    tokio::time::timeout(Duration::from_secs(5), barrier.wait()).await;
                if waited.is_err() {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Body::from(Vec::<u8>::new()),
                    )
                        .into_response();
                }
                (StatusCode::TOO_MANY_REQUESTS, Body::from(zen_bytes)).into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/v1/chat/completions"), calls)
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// opencode-zen key pool (Priority strategy → lowest priority number first),
/// mirroring the observed space-bunny-free configuration: 3 keys sharing one
/// exit IP.
fn zen_pool(keys: &[&str]) -> Arc<KeyPool> {
    let pool = Arc::new(KeyPool::new("opencode-zen", RoutingStrategy::Priority));
    for (i, id) in keys.iter().enumerate() {
        pool.add_key(ApiKeyEntry::new(*id, format!("sk-{id}"), (i + 1) as u32, 10));
    }
    pool
}

fn chat_payload() -> Value {
    json!({
        "model": "space-bunny-free",
        "messages": [{"role": "user", "content": "hello"}]
    })
}

fn zen_429_value() -> Value {
    serde_json::from_str(ZEN_FREE_USAGE_LIMIT_429_BODY).expect("zen 429 body must parse")
}

/// >64 KiB hostile 429 body: 65_450 junk bytes up front, then the zen marker
/// (`FreeUsageLimitError`, ends at byte 65_508 — safely inside the executor's
/// 64 KiB retention window), then a `TRAILER` word + 120_000 junk bytes that
/// begin at byte 65_587 — beyond the cap, so the executor's head-truncation
/// must cut them. Pure ASCII so byte offsets are exact.
fn oversized_zen_429_body() -> String {
    let mut body = String::with_capacity(200 * 1024);
    body.push_str(r#"{"type":"error","error":{"#);
    body.push('"');
    body.extend(std::iter::repeat('A').take(65_450));
    body.push_str(r#"","type":"FreeUsageLimitError","message":"Rate limit exceeded. Please try again later."},"metadata":{"extra":"#);
    body.push_str("TRAILER");
    body.extend(std::iter::repeat('Z').take(120_000));
    body.push_str("\"}}");
    body
}

// ---------------------------------------------------------------------------
// Case 1 — oversized / malformed 429 body: bounded, still quota, still fast-fail
// ---------------------------------------------------------------------------

/// A >64 KiB malformed `FreeUsageLimitError` 429 must be handled without a
/// panic, must be retained under the executor's 64 KiB cap (head-truncation —
/// the junk trailer past the cap is provably cut), and must STILL classify
/// QuotaExhausted and fast-fail after exactly ONE upstream call (JSON path).
#[tokio::test]
async fn oversized_malformed_429_body_bounded_truncation_still_fastfails() {
    // ---- Fixture geometry (guards the byte arithmetic the test relies on) ----
    let body = oversized_zen_429_body();
    assert!(
        body.len() > MAX_UPSTREAM_ERROR_BYTES,
        "fixture: raw body must exceed the 64 KiB cap, got {} bytes",
        body.len()
    );
    let marker_at = body
        .find("FreeUsageLimitError")
        .expect("fixture: zen marker must be embedded");
    assert!(
        marker_at + "FreeUsageLimitError".len() < MAX_UPSTREAM_ERROR_BYTES,
        "fixture: zen marker must sit inside the retention window (found at byte {marker_at})"
    );
    let trailer_at = body
        .find("TRAILER")
        .expect("fixture: trailer word must be embedded");
    assert!(
        trailer_at >= MAX_UPSTREAM_ERROR_BYTES,
        "fixture: trailer must lie beyond the cap so truncation cuts it (found at byte {trailer_at})"
    );

    // ---- Arrange ----
    let server_body = body.clone();
    let (endpoint, _base, calls) = spawn_mock(Arc::new(move |_n, _auth| {
        (StatusCode::TOO_MANY_REQUESTS, server_body.clone().into_bytes())
    }))
    .await;
    let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
    let executor = UpstreamExecutor::new(pool.clone(), 3);

    // ---- Act: must not panic on the hostile body ----
    let err = executor
        .execute_json_request(&endpoint, &chat_payload())
        .await
        .unwrap_err();

    // ---- Assert ----
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            last_error,
            kind,
        } => {
            assert_eq!(
                *kind,
                ponyllm_core::error::GatewayErrorKind::QuotaExhausted,
                "oversized zen body must still classify QuotaExhausted, got {kind:?} (err: {err})"
            );
            assert_eq!(
                *retries, 1,
                "fast-fail must report retries=1 on the hostile body, got {retries} (err: {err})"
            );
            assert_eq!(
                attempted_keys.len(),
                1,
                "fast-fail must attempt exactly one key, got {attempted_keys:?} (err: {err})"
            );
            assert!(
                last_error.contains("(failures: 1 quota exhausted)"),
                "aggregated error must show exactly 1 quota exhaustion, got: {last_error}"
            );
            assert!(
                last_error.contains("FreeUsageLimitError"),
                "zen marker must survive the 64 KiB head-truncation, got: {last_error}"
            );
            assert!(
                !last_error.contains("TRAILER"),
                "bytes beyond the 64 KiB cap must be cut by MAX_UPSTREAM_ERROR_BYTES (head-truncation), got: {last_error}"
            );
            assert!(
                last_error.len() < 75_000,
                "retained error text must be bounded by the 64 KiB cap: raw body was {} bytes, but last_error is {} chars — truncation did not take effect",
                body.len(),
                last_error.len()
            );
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "fast-fail must issue exactly ONE upstream call even for the hostile body, got {}",
        calls.load(Ordering::SeqCst)
    );
}

// ---------------------------------------------------------------------------
// Case 2 — case / wording variants still hit the zen matcher
// ---------------------------------------------------------------------------

/// `is_zen_free_usage_limit_body` (upstream.rs L659-662) lowercases the body
/// and matches `"freeusagelimiterror"` / `"free usage limit"`. This asserts
/// the matcher does NOT depend on the CamelCase spelling, the exact field
/// placement (`error.type`), or JSON structure: an all-lowercase type name and
/// a spaced title-case wording string must both fast-fail in one call.
#[tokio::test]
async fn case_and_wording_variants_still_fastfail_single_call() {
    let variants: [(&str, &str); 2] = [
        (
            "all-lowercase freeusagelimiterror",
            r#"{"type":"error","error":{"type":"freeusagelimiterror","message":"rate limit exceeded. please try again later."},"metadata":{}}"#,
        ),
        (
            "spaced title-case 'Free Usage Limit' wording",
            r#"{"type":"error","error":{"type":"CustomError","message":"Free Usage Limit exhausted for today. Please retry after the window resets."},"metadata":{}}"#,
        ),
    ];

    for (label, raw) in variants {
        let val: Value = serde_json::from_str(raw).expect("fixture: variant body must parse");
        let (endpoint, _base, calls) = spawn_mock(Arc::new(move |_n, _auth| {
            (
                StatusCode::TOO_MANY_REQUESTS,
                serde_json::to_vec(&val).expect("variant body serializes"),
            )
        }))
        .await;
        let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
        let executor = UpstreamExecutor::new(pool.clone(), 3);

        // Act
        let err = executor
            .execute_json_request(&endpoint, &chat_payload())
            .await
            .unwrap_err();

        // Assert — both variants must hit is_zen_free_usage_limit_body.
        match &err {
            CoreError::AllRetriesFailed {
                retries,
                attempted_keys,
                last_error,
                kind,
            } => {
                assert_eq!(
                    *kind,
                    ponyllm_core::error::GatewayErrorKind::QuotaExhausted,
                    "[{label}] must classify QuotaExhausted (zen matcher hit), got {kind:?} (err: {err})"
                );
                assert_eq!(
                    *retries, 1,
                    "[{label}] fast-fail must report retries=1, got {retries} (err: {err})"
                );
                assert_eq!(
                    attempted_keys.len(),
                    1,
                    "[{label}] exactly one attempted key, got {attempted_keys:?}"
                );
                assert!(
                    last_error.contains("(failures: 1 quota exhausted)"),
                    "[{label}] aggregated error must show 1 quota exhaustion, got: {last_error}"
                );
            }
            other => panic!("[{label}] expected AllRetriesFailed, got {other:?}"),
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "[{label}] must fast-fail after exactly ONE upstream call, got {}",
            calls.load(Ordering::SeqCst)
        );
    }
}

// ---------------------------------------------------------------------------
// Case 3 — deterministic concurrency: 4 simultaneous requests, no amplification
// ---------------------------------------------------------------------------

/// 4 identical zen-429 requests fired at the SAME time (tokio::join!) against
/// ONE shared 3-key pool with NO egress pool must each make exactly one
/// upstream call and fast-fail — the total upstream call count must be 4, not
/// 12 (3-key cross-key retry storm was the pre-fix amplification). The
/// server-side rendezvous barrier makes the "simultaneously in flight"
/// interleaving deterministic: all 4 POSTs happen before any 429 can cool a
/// key. No panics.
#[tokio::test]
async fn four_concurrent_zen_429_requests_no_amplification() {
    // Arrange
    const CONCURRENCY: usize = 4;
    let (endpoint, calls) = spawn_rendezvous_zen_429(CONCURRENCY).await;
    let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
    let executor = UpstreamExecutor::new(pool.clone(), 3);
    let payload = chat_payload();

    // Act — all 4 in flight at once; a watchdog binds the worst case so a
    // pathological executor (amplification or premature stop) fails the test
    // instead of hanging the suite.
    let results = tokio::time::timeout(
        Duration::from_secs(30),
        async {
            let (a, b, c, d) = tokio::join!(
                executor.execute_json_request(&endpoint, &payload),
                executor.execute_json_request(&endpoint, &payload),
                executor.execute_json_request(&endpoint, &payload),
                executor.execute_json_request(&endpoint, &payload),
            );
            [a, b, c, d]
        },
    )
    .await
    .expect("the 4 concurrent requests must settle within 30s — hung request or amplification");

    // Assert — every request fast-fails on its own single attempt.
    for (i, res) in results.iter().enumerate() {
        let err = match res {
            Ok(_) => panic!("req {i}: expected a fast-fail error, got Ok"),
            Err(e) => e,
        };
        match err {
            CoreError::AllRetriesFailed {
                retries,
                attempted_keys,
                last_error,
                kind,
            } => {
                assert_eq!(
                    *kind,
                    ponyllm_core::error::GatewayErrorKind::QuotaExhausted,
                    "req {i}: kind must be QuotaExhausted, got {kind:?} (err: {err})"
                );
                assert_eq!(
                    *retries, 1,
                    "req {i}: each request must report retries=1 (single attempt), got {retries} (err: {err})"
                );
                assert_eq!(
                    attempted_keys.len(),
                    1,
                    "req {i}: each request must attempt exactly one key, got {attempted_keys:?}"
                );
                assert!(
                    last_error.contains("(failures: 1 quota exhausted)"),
                    "req {i}: aggregated error must show 1 quota exhaustion, got: {last_error}"
                );
            }
            other => panic!("req {i}: expected AllRetriesFailed, got {other:?}"),
        }
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        CONCURRENCY,
        "no amplification: exactly one upstream call per request — total must be {CONCURRENCY}, got {}",
        calls.load(Ordering::SeqCst)
    );
}