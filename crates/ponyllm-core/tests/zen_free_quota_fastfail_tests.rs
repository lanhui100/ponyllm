//! Wave-3 C7 red-phase acceptance tests — opencode-zen free-quota 429 fast-fail
//! (contract `.dev-team/contracts/wave-3-zen-free-fastfail.md`, frozen by Lead).
//!
//! Scope (Test Agent, task-3): every assertion is written against the FROZEN
//! post-impl contract, then run on the CURRENT (unmodified) executor to prove
//! the red phase:
//!   - C7-1 / C7-2 (zen `FreeUsageLimitError` 429 fast-fail, JSON + stream)
//!     must FAIL on the current code — today the executor makes 3 upstream
//!     calls and reports retries=3 / attempted_keys=[zen-1,zen-2,zen-3];
//!     the contract pins exactly 1 call, retries=1, one attempted key.
//!   - C7-3 / C7-4 / C7-5 (non-zen 429 / egress pool `Some` / FreeTierError
//!     403) must PASS on the current code — the contract freezes "no fast-fail"
//!     for all of them, so they double as regression pins for the executor impl
//!     (must stay green after task-4).
//!   - C7-6 (fast-fail structured WARN with key_id/provider/egress_pool=false)
//!     must FAIL on the current code — no such event exists yet.
//!
//! Infrastructure mirrors `failover_tests.rs`: local axum mock on an ephemeral
//! port (`tokio::net::TcpListener::bind("127.0.0.1:0")`), `axum::Router` +
//! `axum::serve`, `UpstreamExecutor::new(Arc<KeyPool>, retries)` and
//! `.with_egress(Some(Arc<EgressPool>), Some(clients_map))`. KeyPool carries
//! 3 keys (zen-1/zen-2/zen-3) unless noted.
//!
//! Red-phase discipline: every case compiles and runs for real; no `#[ignore]`,
//! no empty bodies, no swallowed errors, no fake assertions (zero-fake-test law).

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{routing::post, Json, Router};
use ponyllm_core::error::CoreError;
use ponyllm_core::executor::*;
use ponyllm_core::pool::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tracing::field::Visit;
use tracing::{span, Event, Id, Metadata, Subscriber};

/// Observed opencode-zen Console 429 (`FreeUsageLimitError`), the body sample
/// pinned by upstream.rs `zen_free_usage_limit_429_is_quota_not_rate_limit`
/// (L3215) and contract C7: despite the "Rate limit exceeded" wording this is a
/// windowed usage quota (`is_zen_free_usage_limit_body` → QuotaExhausted).
const ZEN_FREE_USAGE_LIMIT_429_BODY: &str = r#"{"type":"error","error":{"type":"FreeUsageLimitError","message":"Rate limit exceeded. Please try again later."},"metadata":{}}"#;

/// Plain rate-limit 429 (non-zen body): must NOT trigger fast-fail.
const PLAIN_RATE_LIMIT_429_BODY: &str = r#"{"error":{"message":"rate limit reached","type":"rate_limit_error"}}"#;

/// OpenCode zen free-tier gate 403 (`FreeTierError`): out of contract scope
/// (Non-Goal) — must NOT trigger fast-fail.
const FREE_TIER_403_BODY: &str = r#"{"type":"error","error":{"type":"FreeTierError","message":"OpenCode's free tier can only be used from within OpenCode"}}"#;

/// Spawn a local axum mock on an ephemeral port. Every request is routed to
/// `handler(call_index, authorization_header)`. Returns
/// `(endpoint_url, base_url, call_counter)`. The handler is boxed so the axum
/// route closure captures a concrete, Send + 'static type (the proven
/// `failover_tests.rs` pattern).
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

fn zen_429_value() -> Value {
    serde_json::from_str(ZEN_FREE_USAGE_LIMIT_429_BODY).expect("zen 429 body must parse")
}

fn plain_429_value() -> Value {
    serde_json::from_str(PLAIN_RATE_LIMIT_429_BODY).expect("plain 429 body must parse")
}

fn free_tier_403_value() -> Value {
    serde_json::from_str(FREE_TIER_403_BODY).expect("free tier 403 body must parse")
}

// ---------------------------------------------------------------------------
// C7-1 / C7-2 — zen FreeUsageLimitError 429 with NO egress pool ⇒ fast-fail
// ---------------------------------------------------------------------------

/// C7-1 JSON path + all three fast-fail triggers (429 + zen body + egress_pool
/// None): exactly ONE upstream call, `AllRetriesFailed{kind: QuotaExhausted,
/// retries: 1, attempted_keys: [zen-1]}` and the aggregated error must show
/// "(failures: 1 quota exhausted)". Red on current code (today: 3 calls,
/// retries=3, attempted_keys=[zen-1, zen-2, zen-3]).
#[tokio::test]
async fn c7_1_json_path_zen_free_429_fastfails_after_single_call() {
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

    // Assert — frozen contract C7-1.
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
                "kind must be QuotaExhausted, got {kind:?} (err: {err})"
            );
            assert_eq!(
                *retries, 1,
                "fast-fail must report retries=1 (single attempt), got {retries} (err: {err})"
            );
            assert_eq!(
                attempted_keys.len(),
                1,
                "fast-fail must attempt exactly one key, got {attempted_keys:?} (err: {err})"
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
        "fast-fail must issue exactly ONE upstream call, got {}",
        calls.load(Ordering::SeqCst)
    );
}

/// C7-2 Stream path + the same three triggers: identical fast-fail contract via
/// `execute_stream_request_with_timing_and_key` (the stream 429 branch reads the
/// error body the same way — `resp.bytes()`). Red on current code.
#[tokio::test]
async fn c7_2_stream_path_zen_free_429_fastfails_after_single_call() {
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

    // Assert — frozen contract C7-2 (stream variant).
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
                "kind must be QuotaExhausted, got {kind:?} (err: {err})"
            );
            assert_eq!(
                *retries, 1,
                "fast-fail must report retries=1 (single attempt), got {retries} (err: {err})"
            );
            assert_eq!(
                attempted_keys.len(),
                1,
                "fast-fail must attempt exactly one key, got {attempted_keys:?} (err: {err})"
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
        "stream fast-fail must issue exactly ONE upstream call, got {}",
        calls.load(Ordering::SeqCst)
    );
}

// ---------------------------------------------------------------------------
// C7-3 / C7-4 / C7-5 — NO fast-fail (behavior frozen; green now, green after)
// ---------------------------------------------------------------------------

/// C7-3 Plain (non-zen) rate-limit 429 with no egress pool must NOT trigger
/// fast-fail: current behavior (3 keys → 3 attempts, RateLimit classification,
/// attempted_keys length 3) is the frozen baseline and must stay green after
/// the executor change.
#[tokio::test]
async fn c7_3_non_zen_429_rate_limit_keeps_full_failover() {
    // Arrange
    let plain = plain_429_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::TOO_MANY_REQUESTS, plain.clone())
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

    // Assert — behavior unchanged (green baseline now and after impl).
    match &err {
        CoreError::AllRetriesFailed {
            retries,
            attempted_keys,
            last_error,
            kind,
        } => {
            assert!(
                matches!(kind, ponyllm_core::error::GatewayErrorKind::RateLimitExceeded { .. }),
                "non-zen 429 must keep RateLimit classification, got {kind:?} (err: {err})"
            );
            assert_eq!(
                *retries, 3,
                "no fast-fail: all 3 attempts must run, got {retries} (err: {err})"
            );
            assert_eq!(
                attempted_keys.len(),
                3,
                "no fast-fail: all 3 keys attempted, got {attempted_keys:?}"
            );
            assert!(
                last_error.contains("(failures: 3 rate limited)"),
                "aggregated error must show 3 rate-limited attempts, got: {last_error}"
            );
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "3 upstream calls expected (no fast-fail), got {}",
        calls.load(Ordering::SeqCst)
    );
}

/// C7-4 egress pool = Some (direct + proxy) + zen quota 429 must NOT trigger
/// fast-fail: the 429 cools ONLY the offending exit and the request fails over
/// to the next egress (200). Two mock servers: the `direct` exit dials the
/// endpoint (server A, 429); the `vps` proxy exit routes through server B
/// (a fake forward proxy built with `reqwest::Proxy::all`) which answers 200.
#[tokio::test]
async fn c7_4_egress_pool_some_zen_429_fails_over_to_second_egress() {
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

    let proxy_url = base_b; // e.g. "http://127.0.0.1:PORT" (no path)
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

    // Assert — no fast-fail with an egress pool; cross-egress failover to 200.
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

/// C7-5 FreeTierError 403 (no egress pool) must NOT trigger fast-fail: the 403
/// path is out of contract scope (Non-Goal). Baseline behavior: classified
/// QuotaExhausted via `classify_forbidden`, all 3 keys attempted, 3 upstream
/// calls. Green now and must stay green after the executor change.
#[tokio::test]
async fn c7_5_free_tier_403_keeps_full_failover_no_fastfail() {
    // Arrange
    let free_tier = free_tier_403_value();
    let (endpoint, _base, calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::FORBIDDEN, free_tier.clone())
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

    // Assert — behavior unchanged (green baseline).
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
                "FreeTierError 403 must keep the QuotaExhausted classification, got {kind:?} (err: {err})"
            );
            assert_eq!(
                *retries, 3,
                "403 must NOT fast-fail: 3 attempts, got {retries} (err: {err})"
            );
            assert_eq!(
                attempted_keys.len(),
                3,
                "403 must attempt all keys, got {attempted_keys:?}"
            );
            assert!(
                last_error.contains("(failures: 3 quota exhausted)"),
                "aggregated error must show 3 quota attempts, got: {last_error}"
            );
        }
        other => panic!("expected AllRetriesFailed, got {other:?}"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "3 upstream calls expected (no fast-fail), got {}",
        calls.load(Ordering::SeqCst)
    );
}

// ---------------------------------------------------------------------------
// C7-6 — fast-fail structured WARN (key_id / provider / egress_pool=false)
// ---------------------------------------------------------------------------

/// Minimal `tracing::Subscriber` recording every WARN event's fields as
/// (field-name, debug-value) pairs. No `tracing-subscriber` dependency needed;
/// installed per-test via `tracing::dispatcher::set_default` (thread-local).
/// `#[tokio::test]` runs a current-thread runtime, so every event the executor
/// emits on this thread while the guard is held is captured deterministically.
#[derive(Default)]
struct CapturingSubscriber {
    events: Arc<Mutex<Vec<Vec<(String, String)>>>>,
}

impl Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &span::Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _span: &Id, _values: &span::Record<'_>) {}
    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
    fn event(&self, event: &Event<'_>) {
        if *event.metadata().level() == tracing::Level::WARN {
            struct Collector {
                fields: Vec<(String, String)>,
            }
            impl Visit for Collector {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    self.fields
                        .push((field.name().to_string(), format!("{value:?}")));
                }
            }
            let mut collector = Collector { fields: Vec::new() };
            event.record(&mut collector);
            self.events.lock().unwrap().push(collector.fields);
        }
    }
    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}
}

/// C7-6 Fast-fail structured logging: the contract requires a `tracing::warn!`
/// carrying `key_id`, `provider` and `egress_pool=false` before the fast-fail
/// return. Captured with a real in-process subscriber. Red on current code
/// (no such event exists — the fast-fail return itself does not exist yet).
#[tokio::test]
async fn c7_6_fast_fail_emits_warn_with_key_provider_egress_pool_fields() {
    // Arrange
    let zen_body = zen_429_value();
    let (endpoint, _base, _calls) = spawn_mock_server(Arc::new(move |_n, _auth| {
        (StatusCode::TOO_MANY_REQUESTS, zen_body.clone())
    }))
    .await;
    let pool = zen_pool(&["zen-1", "zen-2", "zen-3"]);
    let executor = UpstreamExecutor::new(pool.clone(), 3);
    let payload = chat_payload();

    let subscriber = CapturingSubscriber::default();
    let captured = subscriber.events.clone();
    let dispatch = tracing::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);

    // Act
    let res = executor.execute_json_request(&endpoint, &payload).await;

    // Assert — red on current code (no fast-fail warn exists yet).
    assert!(
        res.is_err(),
        "the zen 429 must still fail after the fast-fail warn is emitted"
    );
    let events = captured.lock().unwrap();
    let has_field =
        |fields: &[(String, String)], name: &str| fields.iter().any(|(k, _)| k == name);
    let matched: Vec<_> = events
        .iter()
        .filter(|fields| {
            has_field(fields, "key_id")
                && has_field(fields, "provider")
                && has_field(fields, "egress_pool")
        })
        .collect();
    assert!(
        !matched.is_empty(),
        "must capture a WARN event carrying key_id + provider + egress_pool fields; captured {} WARN events: {events:?}",
        events.len()
    );
    assert!(
        matched.iter().any(|fields| {
            fields
                .iter()
                .any(|(k, v)| k == "egress_pool" && v == "false")
        }),
        "egress_pool must be recorded as false (no egress pool configured): {matched:?}"
    );
}
