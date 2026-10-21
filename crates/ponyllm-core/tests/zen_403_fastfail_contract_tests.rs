//! Wave zen-403 red-phase acceptance tests — contract M1/M2 (frozen contract
//! `.dev-team/contracts/2026-10-20-zen-403-hardening.md`, Lead-frozen).
//!
//! Twin-path ruling (contract fact A6 + M1/M2, Lead 2026-10-20): upstream.rs
//! keeps TWO twin implementations of the same 429/403/402 semantics — the JSON
//! line (`execute_json_request_with_key`, ≈:1789-2399) and the streaming line
//! (`execute_stream_request_with_timing_and_key`, ≈:2399-3053+; production runs
//! `stream:true`). M1/M2 MUST land on both lines; every red/green case here is
//! therefore duplicated across the two caller paths.
//!
//! Red-phase discipline: every assertion is pinned to the FROZEN post-impl
//! contract, then run against the CURRENT (unmodified) executor to prove the
//! red phase:
//!   - A-M1-1 / A-M1-1s / A-M1-2 / A-M1-2s (zen FreeTierError 403 + egress
//!     pool → cool the exit + immediate AllRetriesFailed, no cross-key burn)
//!     must FAIL on current code: today the 403 branch only record_error()s
//!     the key and keeps polling → retries=N, N upstream calls, exit NOT
//!     cooled.
//!   - A-M2-2 / A-M2-2s (zen 429 + egress pool len()==1 → fast-fail) must FAIL
//!     on current code: fast-fail fires only when `egress_pool.is_none()`;
//!     with a singleton exit the first 429 cools the exit and the very next
//!     `select_egress` answers NoAvailableKey (attempted_keys grows to 2),
//!     instead of the contract's single-attempt `(failures: 1 …)`.
//!   - A-M1-3(+s) / A-M1-4(+s) / A-M2-1(+s) / A-M2-3(+s) are GREEN regression
//!     anchors (no-trigger scenes keep today's behavior, now and after impl).
//!   - L2-AT: case/JSON variants of the gate body (M1) and of the zen 429 body
//!     (M2) must behave identically to the canonical bodies; a body carrying
//!     the digits "403" but no gate wording (or an empty body) must NOT trip
//!     M1 (no over-trigger, driver-stability boundary).
//!
//! Infrastructure mirrors `zen_free_quota_fastfail_tests.rs` / `failover_tests.rs`:
//! local axum mock on an ephemeral port, `UpstreamExecutor::new(Arc<KeyPool>,
//! retries)` plus `.with_egress(Some(Arc<EgressPool>), clients_map)`. No
//! `#[ignore]`, no empty bodies, real assertions only (zero-fake-test law).

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{routing::post, Json, Router};
use ponyllm_core::error::{CoreError, GatewayErrorKind};
use ponyllm_core::executor::*;
use ponyllm_core::pool::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Observed opencode-zen Console 429 (`FreeUsageLimitError`) — the body sample
/// pinned by `is_zen_free_usage_limit_body` (contract A3 / C7).
const ZEN_FREE_USAGE_LIMIT_429_BODY: &str = r#"{"type":"error","error":{"type":"FreeUsageLimitError","message":"Rate limit exceeded. Please try again later."},"metadata":{}}"#;

/// Observed opencode-zen free-tier gate 403 (`FreeTierError`) — contract A2:
/// `is_zen_free_tier_gate_body` matches `freetiererror` / `free tier can only
/// be used` case-insensitively.
const FREE_TIER_403_BODY: &str = r#"{"type":"error","error":{"type":"FreeTierError","message":"OpenCode's free tier can only be used from within OpenCode"}}"#;

/// Non-zen 403 (balance wording): must keep legacy behavior (A-M1-4) — quota
/// classification on the KEY only, no exit cooldown, no fast-fail.
const BALANCE_403_BODY: &str = r#"{"type":"error","error":{"type":"insufficient_balance","message":"Your account balance is insufficient to cover this request"}}"#;

/// Spawn a local axum mock on an ephemeral port. Every request is routed to
/// `handler(call_index, authorization_header)`. Returns
/// `(endpoint_url, base_url, call_counter)` — the proven C7/failover pattern.
async fn spawn_mock_server(
    handler: Arc<dyn Fn(usize, &str) -> (StatusCode, Value) + Send + Sync>,
) -> (String, String, Arc<AtomicUsize>) {
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
                (status, Json(body)).into_response()
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

/// opencode-zen key pool (Priority strategy → first key selected first),
/// mirroring the observed space-bunny-free configuration.
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

fn free_tier_403_value() -> Value {
    serde_json::from_str(FREE_TIER_403_BODY).expect("free tier 403 body must parse")
}

fn zen_429_value() -> Value {
    serde_json::from_str(ZEN_FREE_USAGE_LIMIT_429_BODY).expect("zen 429 body must parse")
}

fn balance_403_value() -> Value {
    serde_json::from_str(BALANCE_403_BODY).expect("balance 403 body must parse")
}

/// Single `direct` exit pool — mirrors the observed live shape
/// (single exit `egress-home-tencent`, contract A3).
fn single_direct_pool() -> Arc<EgressPool> {
    let eg_pool = Arc::new(EgressPool::new("opencode-zen", EgressStrategy::RoundRobin));
    eg_pool.add_egress(EgressEntry::direct("direct"));
    eg_pool
}

// ---------------------------------------------------------------------------
// M1 — zen FreeTierError 403 + egress pool → cool the exit + immediate
//       AllRetriesFailed (both twin paths).
// ---------------------------------------------------------------------------

/// A-M1-1 (JSON line): zen `FreeTierError` 403 + single-exit egress pool →
/// `AllRetriesFailed{retries:1, kind:QuotaExhausted}`, exactly ONE upstream
/// call, only the current key attempted, the exit cooled to 900s, and the
/// remaining keys NOT burned. Red on current code (today: 3 calls, retries=3,
/// attempted_keys=[zen-1,zen-2,zen-3], exit not cooled, keys 2/3 cooldown 900s).
#[tokio::test]
async fn zen_403_fastfail_am1_1_json_zen_403_single_exit_pool_fastfails_one_call() {
    // Arrange
    let gate = free_tier_403_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::FORBIDDEN, gate.clone())
    }))
    .await;
    let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
    let eg_pool = single_direct_pool();
    let executor = UpstreamExecutor::new(pool.clone(), 3)
        .with_egress(Some(eg_pool.clone()), Some(HashMap::new()));
    let payload = chat_payload();

    // Act
    let err = executor
        .execute_json_request(&endpoint, &payload)
        .await
        .unwrap_err();

    // Assert — frozen contract A-M1-1.
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            last_error,
            kind,
        } => {
            assert_eq!(
                *kind,
                GatewayErrorKind::QuotaExhausted,
                "zen 403 gate must keep QuotaExhausted classification, got {kind:?} (err: {err})"
            );
            assert_eq!(
                *retries, 1,
                "M1: fast-fail must report retries=1 (single attempt), got {retries} (err: {err})"
            );
            assert_eq!(
                attempted_keys.len(),
                1,
                "M1: must NOT poll across keys, got {attempted_keys:?} (err: {err})"
            );
            assert_eq!(
                attempted_keys[0], "zen-1",
                "the only attempted key must be the first Priority key zen-1, got {attempted_keys:?}"
            );
            assert!(
                last_error.contains("(failures: 1 quota exhausted)"),
                "aggregated error must show exactly 1 quota exhaustion, got: {last_error}"
            );
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "M1: exactly ONE upstream call expected, got {}",
        calls.load(Ordering::SeqCst)
    );
    assert!(
        eg_pool.egress_cooldown("direct").0.is_some(),
        "M1: the offending exit must be cooled (record_quota_exhausted), got {:?}",
        eg_pool.egress_cooldown("direct")
    );
    // Key-protection criterion: no cross-key burn — zen-2/zen-3 untouched.
    assert_eq!(
        pool.key_cooldown("zen-2"),
        (None, None),
        "M1: zen-2 must NOT be cooled (contract: no cross-key polling)"
    );
    assert_eq!(
        pool.key_cooldown("zen-3"),
        (None, None),
        "M1: zen-3 must NOT be cooled (contract: no cross-key polling)"
    );
    // Exit cooldown must make the next selection impossible on a singleton.
    assert!(
        matches!(eg_pool.select_egress(), Err(CoreError::NoAvailableKey(_))),
        "M1: with the singleton exit cooling, select_egress must answer NoAvailableKey, got {:?}",
        eg_pool.select_egress()
    );
}

/// A-M1-1s (streaming line, `stream:true` — the production path): identical
/// contract via `execute_stream_request_with_timing_and_key`. Red on current
/// code for the same reasons as the JSON twin.
#[tokio::test]
async fn zen_403_fastfail_am1_1s_stream_zen_403_single_exit_pool_fastfails_one_call() {
    // Arrange
    let gate = free_tier_403_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::FORBIDDEN, gate.clone())
    }))
    .await;
    let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
    let eg_pool = single_direct_pool();
    let executor = UpstreamExecutor::new(pool.clone(), 3)
        .with_egress(Some(eg_pool.clone()), Some(HashMap::new()));
    let payload = chat_payload();

    // Act
    let err = executor
        .execute_stream_request_with_timing_and_key(&endpoint, &payload)
        .await
        .unwrap_err();

    // Assert — frozen contract A-M1-1s (streaming twin).
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            last_error,
            kind,
        } => {
            assert_eq!(
                *kind,
                GatewayErrorKind::QuotaExhausted,
                "streaming 403 gate must keep QuotaExhausted, got {kind:?} (err: {err})"
            );
            assert_eq!(
                *retries, 1,
                "streaming M1: retries=1 expected, got {retries} (err: {err})"
            );
            assert_eq!(
                attempted_keys.len(),
                1,
                "streaming M1: must NOT poll across keys, got {attempted_keys:?} (err: {err})"
            );
            assert_eq!(
                attempted_keys[0], "zen-1",
                "streaming M1: only zen-1 may be attempted, got {attempted_keys:?}"
            );
            assert!(
                last_error.contains("(failures: 1 quota exhausted)"),
                "streaming M1: aggregated error must show 1 quota exhaustion, got: {last_error}"
            );
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "streaming M1: exactly ONE upstream call expected, got {}",
        calls.load(Ordering::SeqCst)
    );
    assert!(
        eg_pool.egress_cooldown("direct").0.is_some(),
        "streaming M1: the offending exit must be cooled, got {:?}",
        eg_pool.egress_cooldown("direct")
    );
    assert_eq!(
        pool.key_cooldown("zen-2"),
        (None, None),
        "streaming M1: zen-2 must NOT be cooled (no cross-key polling)"
    );
    assert!(
        matches!(eg_pool.select_egress(), Err(CoreError::NoAvailableKey(_))),
        "streaming M1: singleton exit cooling ⇒ NoAvailableKey, got {:?}",
        eg_pool.select_egress()
    );
}

/// A-M1-2 (JSON line, multi-exit pool): the FIRST (Priority) exit `direct`
/// answers the zen 403 → M1 cools ONLY `direct` and fast-fails after one call;
/// `select_egress` must SKIP the cooled exit and return the healthy twin
/// (`vps`). Red on current code (today: 3 calls, exit never cooled, selection
/// keeps returning `direct`).
#[tokio::test]
async fn zen_403_fastfail_am1_2_json_zen_403_two_exit_pool_cools_only_offending_exit() {
    // Arrange
    let gate = free_tier_403_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::FORBIDDEN, gate.clone())
    }))
    .await;
    let eg_pool = Arc::new(EgressPool::new("opencode-zen", EgressStrategy::Priority));
    eg_pool.add_egress(EgressEntry::direct("direct"));
    eg_pool.add_egress(EgressEntry::proxy("vps", "http://127.0.0.1:1"));
    let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
    let executor = UpstreamExecutor::new(pool.clone(), 3)
        .with_egress(Some(eg_pool.clone()), Some(HashMap::new()));
    let payload = chat_payload();

    // Act
    let err = executor
        .execute_json_request(&endpoint, &payload)
        .await
        .unwrap_err();

    // Assert — frozen contract A-M1-2.
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            last_error,
            kind,
        } => {
            assert_eq!(
                *kind,
                GatewayErrorKind::QuotaExhausted,
                "kind must be QuotaExhausted, got {kind:?} (err: {err})"
            );
            assert_eq!(
                *retries, 1,
                "M1: single attempt expected, got {retries} (err: {err})"
            );
            assert_eq!(
                attempted_keys.len(),
                1,
                "M1: no cross-key polling, got {attempted_keys:?} (err: {err})"
            );
            assert!(
                last_error.contains("(failures: 1 quota exhausted)"),
                "aggregated error must show 1 quota exhaustion, got: {last_error}"
            );
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "M1: exactly ONE upstream call, got {}",
        calls.load(Ordering::SeqCst)
    );
    assert!(
        eg_pool.egress_cooldown("direct").0.is_some(),
        "M1: ONLY the offending exit may be cooled, direct got {:?}",
        eg_pool.egress_cooldown("direct")
    );
    assert_eq!(
        eg_pool.egress_cooldown("vps").0,
        None,
        "M1: the untouched exit must stay active"
    );
    // Selection skips the cooled exit → healthy twin is returned next.
    let next = eg_pool.select_egress().expect("vps must remain selectable");
    assert_eq!(
        next.id, "vps",
        "M1: select_egress must SKIP the cooled direct exit, got {:?}",
        next.id
    );
}

/// A-M1-2s (streaming twin of A-M1-2): identical contract via the streaming
/// caller. Red on current code.
#[tokio::test]
async fn zen_403_fastfail_am1_2s_stream_zen_403_two_exit_pool_cools_only_offending_exit() {
    // Arrange
    let gate = free_tier_403_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::FORBIDDEN, gate.clone())
    }))
    .await;
    let eg_pool = Arc::new(EgressPool::new("opencode-zen", EgressStrategy::Priority));
    eg_pool.add_egress(EgressEntry::direct("direct"));
    eg_pool.add_egress(EgressEntry::proxy("vps", "http://127.0.0.1:1"));
    let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
    let executor = UpstreamExecutor::new(pool.clone(), 3)
        .with_egress(Some(eg_pool.clone()), Some(HashMap::new()));
    let payload = chat_payload();

    // Act
    let err = executor
        .execute_stream_request_with_timing_and_key(&endpoint, &payload)
        .await
        .unwrap_err();

    // Assert — frozen contract A-M1-2s (streaming twin).
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            last_error,
            kind,
        } => {
            assert_eq!(
                *kind,
                GatewayErrorKind::QuotaExhausted,
                "kind must be QuotaExhausted, got {kind:?} (err: {err})"
            );
            assert_eq!(
                *retries, 1,
                "streaming M1: single attempt expected, got {retries} (err: {err})"
            );
            assert_eq!(
                attempted_keys.len(),
                1,
                "streaming M1: no cross-key polling, got {attempted_keys:?} (err: {err})"
            );
            assert!(
                last_error.contains("(failures: 1 quota exhausted)"),
                "aggregated error must show 1 quota exhaustion, got: {last_error}"
            );
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "streaming M1: exactly ONE upstream call, got {}",
        calls.load(Ordering::SeqCst)
    );
    assert!(
        eg_pool.egress_cooldown("direct").0.is_some(),
        "streaming M1: ONLY direct may be cooled, got {:?}",
        eg_pool.egress_cooldown("direct")
    );
    assert_eq!(eg_pool.egress_cooldown("vps").0, None, "vps must stay active");
    let next = eg_pool.select_egress().expect("vps must remain selectable");
    assert_eq!(
        next.id, "vps",
        "streaming M1: select_egress must skip the cooled direct exit, got {:?}",
        next.id
    );
}

// ---------------------------------------------------------------------------
// M1 no-trigger regression anchors (GREEN now, must stay green after impl)
// ---------------------------------------------------------------------------

/// A-M1-3 (JSON line): zen 403 WITHOUT an egress pool must NOT fast-fail —
/// legacy full failover (2 keys → 2 attempts / 2 calls) is the frozen
/// baseline. Green now and after (M1 is keyed on `egress_pool == Some`).
#[tokio::test]
async fn zen_403_fastfail_am1_3_json_zen_403_no_egress_pool_keeps_full_failover() {
    // Arrange
    let gate = free_tier_403_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::FORBIDDEN, gate.clone())
    }))
    .await;
    let pool = zen_pool(&["zen-1", "zen-2"]);
    let executor = UpstreamExecutor::new(pool.clone(), 2);
    let payload = chat_payload();

    // Act
    let err = executor
        .execute_json_request(&endpoint, &payload)
        .await
        .unwrap_err();

    // Assert — no fast-fail without an egress pool (A-M1-3).
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            ..
        } => {
            assert_eq!(
                *retries, 2,
                "no egress pool ⇒ no fast-fail: 2 attempts, got {retries} (err: {err})"
            );
            assert_eq!(
                attempted_keys.len(),
                2,
                "both keys must be attempted, got {attempted_keys:?}"
            );
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2, "2 calls expected, got {}", calls.load(Ordering::SeqCst));
}

/// A-M1-3s (streaming twin): same no-trigger anchor on the streaming line.
#[tokio::test]
async fn zen_403_fastfail_am1_3s_stream_zen_403_no_egress_pool_keeps_full_failover() {
    // Arrange
    let gate = free_tier_403_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::FORBIDDEN, gate.clone())
    }))
    .await;
    let pool = zen_pool(&["zen-1", "zen-2"]);
    let executor = UpstreamExecutor::new(pool.clone(), 2);
    let payload = chat_payload();

    // Act
    let err = executor
        .execute_stream_request_with_timing_and_key(&endpoint, &payload)
        .await
        .unwrap_err();

    // Assert — no fast-fail without an egress pool (A-M1-3s).
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            ..
        } => {
            assert_eq!(
                *retries, 2,
                "streaming: no egress pool ⇒ 2 attempts, got {retries} (err: {err})"
            );
            assert_eq!(attempted_keys.len(), 2, "both keys attempted, got {attempted_keys:?}");
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2, "2 calls expected, got {}", calls.load(Ordering::SeqCst));
}

/// A-M1-4 (JSON line): NON-zen 403 (balance wording) WITH an egress pool must
/// NOT fast-fail and must NOT cool the exit — legacy `classify_forbidden`
/// semantics only (key-level record_error, full failover). Green now/after.
#[tokio::test]
async fn zen_403_fastfail_am1_4_json_non_zen_403_with_pool_no_fastfail_no_exit_cooldown() {
    // Arrange
    let bal = balance_403_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::FORBIDDEN, bal.clone())
    }))
    .await;
    let pool = zen_pool(&["zen-1", "zen-2"]);
    let eg_pool = single_direct_pool();
    let executor = UpstreamExecutor::new(pool.clone(), 2)
        .with_egress(Some(eg_pool.clone()), Some(HashMap::new()));
    let payload = chat_payload();

    // Act
    let err = executor
        .execute_json_request(&endpoint, &payload)
        .await
        .unwrap_err();

    // Assert — non-zen 403 keeps today's behavior (A-M1-4).
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            ..
        } => {
            assert_eq!(
                *retries, 2,
                "non-zen 403: no fast-fail, 2 attempts, got {retries} (err: {err})"
            );
            assert_eq!(attempted_keys.len(), 2, "both keys attempted, got {attempted_keys:?}");
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2, "2 calls expected, got {}", calls.load(Ordering::SeqCst));
    assert_eq!(
        eg_pool.egress_cooldown("direct").0,
        None,
        "non-zen 403 must NOT cool the exit (M1 is zen-gate-only)"
    );
}

/// A-M1-4s (streaming twin): same no-trigger anchor on the streaming line.
#[tokio::test]
async fn zen_403_fastfail_am1_4s_stream_non_zen_403_with_pool_no_fastfail_no_exit_cooldown() {
    // Arrange
    let bal = balance_403_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::FORBIDDEN, bal.clone())
    }))
    .await;
    let pool = zen_pool(&["zen-1", "zen-2"]);
    let eg_pool = single_direct_pool();
    let executor = UpstreamExecutor::new(pool.clone(), 2)
        .with_egress(Some(eg_pool.clone()), Some(HashMap::new()));
    let payload = chat_payload();

    // Act
    let err = executor
        .execute_stream_request_with_timing_and_key(&endpoint, &payload)
        .await
        .unwrap_err();

    // Assert — non-zen 403 keeps today's behavior (A-M1-4s).
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            ..
        } => {
            assert_eq!(
                *retries, 2,
                "streaming: non-zen 403 no fast-fail, 2 attempts, got {retries} (err: {err})"
            );
            assert_eq!(attempted_keys.len(), 2, "both keys attempted, got {attempted_keys:?}");
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2, "2 calls expected, got {}", calls.load(Ordering::SeqCst));
    assert_eq!(
        eg_pool.egress_cooldown("direct").0,
        None,
        "streaming: non-zen 403 must NOT cool the exit"
    );
}

// ---------------------------------------------------------------------------
// M2 — zen 429 fast-fail when the egress pool is absent OR len() <= 1
// ---------------------------------------------------------------------------

/// A-M2-1 (JSON line, wave-3 C7 regression anchor): zen 429 + NO egress pool
/// already fast-fails today (`egress_pool.is_none()`). Green now, must stay
/// green after M2 widens the trigger.
#[tokio::test]
async fn zen_403_fastfail_am2_1_json_zen_429_no_pool_fastfails() {
    // Arrange
    let zen_body = zen_429_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::TOO_MANY_REQUESTS, zen_body.clone())
    }))
    .await;
    let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
    let executor = UpstreamExecutor::new(pool.clone(), 3);
    let payload = chat_payload();

    // Act
    let err = executor
        .execute_json_request(&endpoint, &payload)
        .await
        .unwrap_err();

    // Assert — C7-1 contract keeps holding (A-M2-1).
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            last_error,
            kind,
        } => {
            assert_eq!(*kind, GatewayErrorKind::QuotaExhausted, "got {kind:?} (err: {err})");
            assert_eq!(*retries, 1, "got {retries} (err: {err})");
            assert_eq!(attempted_keys.len(), 1, "got {attempted_keys:?}");
            assert!(
                last_error.contains("(failures: 1 quota exhausted)"),
                "got: {last_error}"
            );
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "got {}", calls.load(Ordering::SeqCst));
}

/// A-M2-1s (streaming twin of C7-2): same regression anchor on the streaming line.
#[tokio::test]
async fn zen_403_fastfail_am2_1s_stream_zen_429_no_pool_fastfails() {
    // Arrange
    let zen_body = zen_429_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::TOO_MANY_REQUESTS, zen_body.clone())
    }))
    .await;
    let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
    let executor = UpstreamExecutor::new(pool.clone(), 3);
    let payload = chat_payload();

    // Act
    let err = executor
        .execute_stream_request_with_timing_and_key(&endpoint, &payload)
        .await
        .unwrap_err();

    // Assert — C7-2 contract keeps holding (A-M2-1s).
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            last_error,
            kind,
        } => {
            assert_eq!(*kind, GatewayErrorKind::QuotaExhausted, "got {kind:?} (err: {err})");
            assert_eq!(*retries, 1, "got {retries} (err: {err})");
            assert_eq!(attempted_keys.len(), 1, "got {attempted_keys:?}");
            assert!(
                last_error.contains("(failures: 1 quota exhausted)"),
                "got: {last_error}"
            );
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "got {}", calls.load(Ordering::SeqCst));
}

/// A-M2-2 (JSON line): zen 429 + egress pool len()==1 must NOW fast-fail
/// (single shared IP bucket ⇒ cross-key polling is meaningless amplification).
/// Exit cooldown accounting (`:2171-2177` twin) is preserved. Red on current
/// code: today the first 429 cools the exit, the second attempt hits
/// `select_egress → NoAvailableKey`, surfacing
/// `AllRetriesFailed{retries:1, attempted_keys:[zen-1,zen-2]}` — never the
/// contract's single-key `(failures: 1 …)` fast-fail.
#[tokio::test]
async fn zen_403_fastfail_am2_2_json_zen_429_single_exit_pool_fastfails() {
    // Arrange
    let zen_body = zen_429_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::TOO_MANY_REQUESTS, zen_body.clone())
    }))
    .await;
    let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
    let eg_pool = single_direct_pool();
    let executor = UpstreamExecutor::new(pool.clone(), 3)
        .with_egress(Some(eg_pool.clone()), Some(HashMap::new()));
    let payload = chat_payload();

    // Act
    let err = executor
        .execute_json_request(&endpoint, &payload)
        .await
        .unwrap_err();

    // Assert — frozen contract A-M2-2: M2 extends fast-fail to len()<=1.
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            last_error,
            kind,
        } => {
            assert_eq!(
                *kind,
                GatewayErrorKind::QuotaExhausted,
                "kind must be QuotaExhausted, got {kind:?} (err: {err})"
            );
            assert_eq!(
                *retries, 1,
                "M2: singleton exit ⇒ fast-fail with retries=1, got {retries} (err: {err})"
            );
            assert_eq!(
                attempted_keys.len(),
                1,
                "M2: no cross-key polling on a singleton exit, got {attempted_keys:?} (err: {err})"
            );
            assert_eq!(
                attempted_keys[0], "zen-1",
                "only zen-1 may be attempted, got {attempted_keys:?}"
            );
            assert!(
                last_error.contains("(failures: 1 quota exhausted)"),
                "aggregated error must show exactly 1 quota exhaustion, got: {last_error}"
            );
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "M2: exactly ONE upstream call, got {}",
        calls.load(Ordering::SeqCst)
    );
    assert!(
        eg_pool.egress_cooldown("direct").0.is_some(),
        "M2: the 429 exit-cooldown accounting (:2171-2177 twin) must be preserved, got {:?}",
        eg_pool.egress_cooldown("direct")
    );
}

/// A-M2-2s (streaming twin): same M2 contract via the streaming caller. Red on
/// current code (streaming 429 branch `:2848` still keys on `is_none()` only).
#[tokio::test]
async fn zen_403_fastfail_am2_2s_stream_zen_429_single_exit_pool_fastfails() {
    // Arrange
    let zen_body = zen_429_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::TOO_MANY_REQUESTS, zen_body.clone())
    }))
    .await;
    let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
    let eg_pool = single_direct_pool();
    let executor = UpstreamExecutor::new(pool.clone(), 3)
        .with_egress(Some(eg_pool.clone()), Some(HashMap::new()));
    let payload = chat_payload();

    // Act
    let err = executor
        .execute_stream_request_with_timing_and_key(&endpoint, &payload)
        .await
        .unwrap_err();

    // Assert — frozen contract A-M2-2s (streaming twin).
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            last_error,
            kind,
        } => {
            assert_eq!(
                *kind,
                GatewayErrorKind::QuotaExhausted,
                "streaming kind must be QuotaExhausted, got {kind:?} (err: {err})"
            );
            assert_eq!(
                *retries, 1,
                "streaming M2: retries=1 expected, got {retries} (err: {err})"
            );
            assert_eq!(
                attempted_keys.len(),
                1,
                "streaming M2: no cross-key polling on a singleton exit, got {attempted_keys:?} (err: {err})"
            );
            assert_eq!(
                attempted_keys[0], "zen-1",
                "streaming M2: only zen-1 may be attempted, got {attempted_keys:?}"
            );
            assert!(
                last_error.contains("(failures: 1 quota exhausted)"),
                "streaming M2: aggregated error must show 1 quota exhaustion, got: {last_error}"
            );
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "streaming M2: exactly ONE upstream call, got {}",
        calls.load(Ordering::SeqCst)
    );
    assert!(
        eg_pool.egress_cooldown("direct").0.is_some(),
        "streaming M2: exit-cooldown accounting must be preserved, got {:?}",
        eg_pool.egress_cooldown("direct")
    );
}

/// A-M2-3 (JSON line, boundary pin): zen 429 + egress pool len()==2 must NOT
/// fast-fail — the 429 cools ONLY the offending exit and the request fails
/// over to the second egress (200). Two mock servers: the `direct` exit dials
/// server A (429); the `vps` proxy exit routes through server B (a fake
/// forward proxy built with `reqwest::Proxy::all`) which answers 200. Green
/// now and must stay green after M2 (len()>=2 keeps the failover path).
#[tokio::test]
async fn zen_403_fastfail_am2_3_json_zen_429_two_exit_pool_fails_over() {
    // Arrange
    let zen_body = zen_429_value();
    let (endpoint, _base_a, a_calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::TOO_MANY_REQUESTS, zen_body.clone())
    }))
    .await;
    let (_endpoint_b, base_b, b_calls) = spawn_mock_server(Arc::new(|_n, _auth| {
        (
            StatusCode::OK,
            json!({
                "id": "chatcmpl-egress-2",
                "object": "chat.completion",
                "created": 1710000000,
                "model": "space-bunny-free",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "Success via second egress!"},
                    "finish_reason": "stop"
                }]
            }),
        )
    }))
    .await;

    let proxy_url = base_b; // "http://127.0.0.1:PORT" (no path)
    let proxy_client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(proxy_url.clone()).expect("proxy url must parse"))
        .build()
        .expect("proxy client must build");
    let clients_map = HashMap::from([(proxy_url.clone(), proxy_client)]);

    let eg_pool = Arc::new(EgressPool::new("opencode-zen", EgressStrategy::RoundRobin));
    eg_pool.add_egress(EgressEntry::direct("direct"));
    eg_pool.add_egress(EgressEntry::proxy("vps", proxy_url.clone()));

    let pool = zen_pool(&["zen-1", "zen-2"]);
    let executor = UpstreamExecutor::new(pool.clone(), 2)
        .with_egress(Some(eg_pool.clone()), Some(clients_map));
    let payload = chat_payload();

    // Act
    let resp = executor
        .execute_json_request(&endpoint, &payload)
        .await
        .unwrap();

    // Assert — no fast-fail with len()==2; cross-egress failover to 200.
    assert_eq!(
        resp["choices"][0]["message"]["content"],
        "Success via second egress!",
        "must succeed on the second egress, got: {resp}"
    );
    assert_eq!(
        a_calls.load(Ordering::SeqCst),
        1,
        "direct exit: exactly one quota-429 call, got {}",
        a_calls.load(Ordering::SeqCst)
    );
    assert_eq!(
        b_calls.load(Ordering::SeqCst),
        1,
        "proxy exit: exactly one (winning) call, got {}",
        b_calls.load(Ordering::SeqCst)
    );
    assert!(
        eg_pool.egress_cooldown("direct").0.is_some(),
        "the quota 429 must cool ONLY the direct exit"
    );
    assert_eq!(
        eg_pool.egress_cooldown("vps").0,
        None,
        "the second exit must stay untouched"
    );
}

/// A-M2-3s (streaming twin of the len()==2 boundary): same no-fast-fail
/// failover contract on the streaming line.
#[tokio::test]
async fn zen_403_fastfail_am2_3s_stream_zen_429_two_exit_pool_fails_over() {
    // Arrange
    let zen_body = zen_429_value();
    let (endpoint, _base_a, a_calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::TOO_MANY_REQUESTS, zen_body.clone())
    }))
    .await;
    let (_endpoint_b, base_b, b_calls) = spawn_mock_server(Arc::new(|_n, _auth| {
        (
            StatusCode::OK,
            json!({
                "id": "chatcmpl-egress-2",
                "object": "chat.completion",
                "created": 1710000000,
                "model": "space-bunny-free",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "Success via second egress!"},
                    "finish_reason": "stop"
                }]
            }),
        )
    }))
    .await;

    let proxy_url = base_b;
    let proxy_client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(proxy_url.clone()).expect("proxy url must parse"))
        .build()
        .expect("proxy client must build");
    let clients_map = HashMap::from([(proxy_url.clone(), proxy_client)]);

    let eg_pool = Arc::new(EgressPool::new("opencode-zen", EgressStrategy::RoundRobin));
    eg_pool.add_egress(EgressEntry::direct("direct"));
    eg_pool.add_egress(EgressEntry::proxy("vps", proxy_url.clone()));

    let pool = zen_pool(&["zen-1", "zen-2"]);
    let executor = UpstreamExecutor::new(pool.clone(), 2)
        .with_egress(Some(eg_pool.clone()), Some(clients_map));
    let payload = chat_payload();

    // Act — the streaming caller returns the raw response tuple on success.
    let (resp, _elapsed, key_id) = executor
        .execute_stream_request_with_timing_and_key(&endpoint, &payload)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&resp.bytes().await.expect("body readable"))
        .expect("200 body must parse as JSON");

    // Assert — streaming twin of A-M2-3: no fast-fail, failover to the 2nd exit.
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "Success via second egress!",
        "must succeed on the second egress, got: {body}"
    );
    assert_eq!(key_id, "zen-2", "streaming success reports the winning key, got {key_id}");
    assert_eq!(a_calls.load(Ordering::SeqCst), 1, "direct: 1 quota-429 call, got {}", a_calls.load(Ordering::SeqCst));
    assert_eq!(b_calls.load(Ordering::SeqCst), 1, "proxy: 1 winning call, got {}", b_calls.load(Ordering::SeqCst));
    assert!(
        eg_pool.egress_cooldown("direct").0.is_some(),
        "the quota 429 must cool ONLY the direct exit"
    );
    assert_eq!(eg_pool.egress_cooldown("vps").0, None, "the second exit must stay untouched");
}

// ---------------------------------------------------------------------------
// L2-AT adversarial — malformed / boundary bodies, deterministic fixtures
// ---------------------------------------------------------------------------

/// L2-AT (M1): zen gate bodies in hostile case / wording / JSON variants must
/// behave EXACTLY like the canonical `FreeTierError` body — fast-fail on the
/// prefix key, cool the singleton exit, ONE upstream call. Every variant is
/// `is_zen_free_tier_gate_body`-matched (contract A2), so M1 applies. Red on
/// current code (all variants: 3 calls, exit never cooled).
#[tokio::test]
async fn zen_403_fastfail_lat_m1_gate_body_case_and_json_variants_fastfail() {
    let variants: Vec<(&str, Value)> = vec![
        ("canonical FreeTierError", free_tier_403_value()),
        (
            "lowercase freetiererror",
            serde_json::from_str(r#"{"type":"error","error":{"type":"freetiererror","message":"OpenCode's free tier can only be used from within OpenCode"}}"#).unwrap(),
        ),
        (
            "UPPERCASE FREETIERERROR",
            serde_json::from_str(r#"{"type":"error","error":{"type":"FREETIERERROR","message":"OpenCode's free tier can only be used from within OpenCode"}}"#).unwrap(),
        ),
        (
            "mixed-case FrEeTiErRoRr",
            serde_json::from_str(r#"{"type":"error","error":{"type":"FrEeTiErRoRr","message":"OpenCode's free tier can only be used from within OpenCode"}}"#).unwrap(),
        ),
        (
            "plain wording (no JSON type field)",
            json!({"error": "free tier can only be used from within OpenCode"}),
        ),
        (
            "title-case wording",
            json!({"error": {"message": "Free Tier Can Only Be Used From Within OpenCode"}}),
        ),
        (
            "json-escaped gate text + junk fields",
            json!({
                "status": 403,
                "error": {"type": "Error", "message": "{\"type\":\"FreeTierError\"}"},
                "extra": ["junk", 1, {"k": "v"}]
            }),
        ),
        (
            "403 digit as JSON value + gate text (mixed)",
            json!({"status": 403, "code": 403, "error": {"type": "FreeTierError", "message": "403: free tier blocked"}}),
        ),
    ];

    for (label, body) in variants {
        // Arrange — fresh server / pool / exit per variant (no cross-talk).
        let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
            (StatusCode::FORBIDDEN, body.clone())
        }))
        .await;
        let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
        let eg_pool = single_direct_pool();
        let executor = UpstreamExecutor::new(pool.clone(), 3)
            .with_egress(Some(eg_pool.clone()), Some(HashMap::new()));
        let payload = chat_payload();

        // Act
        let err = executor
            .execute_json_request(&endpoint, &payload)
            .await
            .unwrap_err();

        // Assert — every gate variant gets the M1 treatment.
        match &err {
            CoreError::AllRetriesFailed {
                retries,
                attempted_keys,
                last_error,
                kind,
            } => {
                assert_eq!(
                    *kind,
                    GatewayErrorKind::QuotaExhausted,
                    "[{label}] must classify QuotaExhausted, got {kind:?} (err: {err})"
                );
                assert_eq!(
                    *retries, 1,
                    "[{label}] M1 fast-fail must report retries=1, got {retries} (err: {err})"
                );
                assert_eq!(
                    attempted_keys.len(),
                    1,
                    "[{label}] must attempt exactly one key, got {attempted_keys:?} (err: {err})"
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
            "[{label}] M1 must issue exactly ONE upstream call, got {}",
            calls.load(Ordering::SeqCst)
        );
        assert!(
            eg_pool.egress_cooldown("direct").0.is_some(),
            "[{label}] the exit must be cooled, got {:?}",
            eg_pool.egress_cooldown("direct")
        );
        assert!(
            matches!(eg_pool.select_egress(), Err(CoreError::NoAvailableKey(_))),
            "[{label}] cooled singleton exit ⇒ NoAvailableKey, got {:?}",
            eg_pool.select_egress()
        );
    }
}

/// L2-AT (M1 no-over-trigger boundary): a 403 body carrying the digits "403"
/// but NO gate wording — and an EMPTY body — must NOT trip M1. The zen-gate
/// predicate (`is_zen_free_tier_gate_body`, contract A2) is the only trigger;
/// these fall to the legacy unknown-403 branch (key-level 60s cooldown, full
/// failover) with NO exit cooldown and NO fast-fail. Green now and after
/// (driver-stability: the new branch must not over-fire on lookalike bodies).
#[tokio::test]
async fn zen_403_fastfail_lat_m1_403_digit_non_gate_body_no_fastfail() {
    let bodies: Vec<(&str, Value)> = vec![
        // Contains the digits "403" as JSON values, but no gate/balance/rate
        // wording — an adversarial lookalike of the real gate rejection.
        (
            "403 digit in body without gate wording",
            json!({"status": 403, "code": 403, "error": {"message": "Forbidden"}}),
        ),
        // Empty error body (transport-truncated upstream reply).
        ("empty error body", Value::Null),
    ];

    for (label, body) in bodies {
        // Arrange — fresh server / pool / exit per variant.
        let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
            (StatusCode::FORBIDDEN, body.clone())
        }))
        .await;
        let pool = zen_pool(&["zen-1", "zen-2"]);
        let eg_pool = single_direct_pool();
        let executor = UpstreamExecutor::new(pool.clone(), 2)
            .with_egress(Some(eg_pool.clone()), Some(HashMap::new()));
        let payload = chat_payload();

        // Act
        let err = executor
            .execute_json_request(&endpoint, &payload)
            .await
            .unwrap_err();

        // Assert — legacy unknown-403 behavior, zero M1 triggering.
        match &err {
            CoreError::AllRetriesFailed {
                retries,
                attempted_keys,
                ..
            } => {
                assert_eq!(
                    *retries, 2,
                    "[{label}] non-gate 403 must NOT fast-fail: 2 attempts, got {retries} (err: {err})"
                );
                assert_eq!(
                    attempted_keys.len(),
                    2,
                    "[{label}] both keys must be attempted, got {attempted_keys:?}"
                );
            }
            other => panic!("[{label}] expected AllRetriesFailed, got {other:?}"),
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "[{label}] 2 upstream calls expected, got {}",
            calls.load(Ordering::SeqCst)
        );
        assert_eq!(
            eg_pool.egress_cooldown("direct").0,
            None,
            "[{label}] M1 must NOT cool the exit for a non-gate body"
        );
    }
}

/// L2-AT (M2): zen 429 bodies in hostile case / wording variants must fast-fail
/// identically on a singleton-exit pool (all matched by
/// `is_zen_free_usage_limit_body`, contract A3). Red on current code (today the
/// singleton exit cools on the first 429 and the next `select_egress` answers
/// NoAvailableKey with attempted_keys length 2 — never the single-attempt
/// `(failures: 1 …)` fast-fail that M2 freezes).
#[tokio::test]
async fn zen_403_fastfail_lat_m2_zen_429_body_variants_single_exit_fastfail() {
    let variants: Vec<(&str, Value)> = vec![
        ("canonical FreeUsageLimitError", zen_429_value()),
        (
            "lowercase freeusagelimiterror",
            serde_json::from_str(r#"{"type":"error","error":{"type":"freeusagelimiterror","message":"Rate limit exceeded. Please try again later."}}"#).unwrap(),
        ),
        (
            "UPPERCASE FREEUSAGELIMITERROR",
            serde_json::from_str(r#"{"type":"error","error":{"type":"FREEUSAGELIMITERROR","message":"Rate limit exceeded. Please try again later."}}"#).unwrap(),
        ),
        (
            "plain wording Free Usage Limit",
            json!({"error": {"message": "Free usage limit reached. Please try again later."}}),
        ),
    ];

    for (label, body) in variants {
        // Arrange — fresh server / pool / exit per variant (no cross-talk).
        let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
            (StatusCode::TOO_MANY_REQUESTS, body.clone())
        }))
        .await;
        let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
        let eg_pool = single_direct_pool();
        let executor = UpstreamExecutor::new(pool.clone(), 3)
            .with_egress(Some(eg_pool.clone()), Some(HashMap::new()));
        let payload = chat_payload();

        // Act
        let err = executor
            .execute_json_request(&endpoint, &payload)
            .await
            .unwrap_err();

        // Assert — every matched zen 429 variant gets the M2 fast-fail.
        match &err {
            CoreError::AllRetriesFailed {
                retries,
                attempted_keys,
                last_error,
                kind,
            } => {
                assert_eq!(
                    *kind,
                    GatewayErrorKind::QuotaExhausted,
                    "[{label}] must classify QuotaExhausted, got {kind:?} (err: {err})"
                );
                assert_eq!(
                    *retries, 1,
                    "[{label}] M2 fast-fail must report retries=1, got {retries} (err: {err})"
                );
                assert_eq!(
                    attempted_keys.len(),
                    1,
                    "[{label}] must attempt exactly one key, got {attempted_keys:?} (err: {err})"
                );
                assert_eq!(
                    attempted_keys[0], "zen-1",
                    "[{label}] only zen-1 may be attempted, got {attempted_keys:?}"
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
            "[{label}] M2 must issue exactly ONE upstream call, got {}",
            calls.load(Ordering::SeqCst)
        );
        assert!(
            eg_pool.egress_cooldown("direct").0.is_some(),
            "[{label}] the 429 exit-cooldown accounting must be preserved, got {:?}",
            eg_pool.egress_cooldown("direct")
        );
    }
}