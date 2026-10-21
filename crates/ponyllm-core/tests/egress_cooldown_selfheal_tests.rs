//! Wave-1 egress self-heal L2-T functional contract tests (contract
//! `.dev-team/contracts/2026-10-22-egress-cooldown-selfheal.md`, frozen by Lead;
//! task-2, Test Agent).
//!
//! Phase history: written as RED phase against the `unimplemented!()` stubs
//! (egress.rs L353/L361/L368) — all cases panicked, exit 101. The impl landed
//! (0ca6609 / ef932cd) and the frozen suite is now fully green; T7 was added
//! per the L3-B review to cover the executor inline-probe path.
//!
//! T6 (stream twin, C8): supported — `execute_stream_request_with_timing_and_key`
//! (upstream.rs:2502) returns the raw `reqwest::Response` and fails fast on
//! selection errors exactly like the JSON path, so the mock needs no SSE body;
//! the twin asserts unlock + success and cooldown-kept + fail-fast on the
//! stream executor path.
//!
//! T7 (L3-B review coverage): the EXECUTOR's inline recovery probe
//! (upstream.rs:1385 `egress_recovery_probe`) — an all-cooled
//! `execute_json_request` must itself probe GET {base}/models, unlock the exit
//! and reselect, then POST the chat to success (no manual API driving;
//! complements the manual-drive T1).
//!
//! The L2-AT adversarial suite (contract path
//! `tests/acceptance/adversarial/egress_selfheal/`) is wired in below via
//! `#[path]`, so the whole wave-1 self-heal suite runs with one command.
//!
//! Discipline: no `#[ignore]`, no empty bodies, no swallowed errors, no fake
//! assertions; assertions are frozen against the contract and run on the
//! CURRENT (unmodified) executor to prove the red phase.

/// L2-AT adversarial module (A1 timeout / A2 malformed probe / A3 concurrent
/// single-probe / A4 inline stalled-body) — file lives at the frozen contract
/// path and is included here so cargo runs it
/// (`cargo test -p ponyllm-core egress_cooldown_selfheal`).
#[path = "../../../tests/acceptance/adversarial/egress_selfheal/egress_selfheal_adversarial_tests.rs"]
mod egress_selfheal_l2at;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{routing::{get, post}, Router};
use ponyllm_core::error::CoreError;
use ponyllm_core::executor::*;
use ponyllm_core::pool::egress::PROBE_INTERVAL;
use ponyllm_core::pool::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Observed opencode-zen Console 429 (`FreeUsageLimitError`) — the body the
/// gateway receives when a free-quota exit is exhausted (contract background;
/// same sample pinned by `zen_free_quota_fastfail_tests.rs`).
const ZEN_FREE_USAGE_LIMIT_429_BODY: &str = r#"{"type":"error","error":{"type":"FreeUsageLimitError","message":"Rate limit exceeded. Please try again later."},"metadata":{}}"#;

/// Local axum mock (own version, dual route): GET /models = recovery-probe
/// surface (C7 payload), POST /v1/chat/completions = chat surface. Handlers
/// receive the per-route call index and return (status, raw body). Returns
/// `(endpoint_url, base_url, models_calls, chat_calls)`.
async fn spawn_selfheal_mock(
    models: Arc<dyn Fn(usize) -> (StatusCode, String) + Send + Sync>,
    chat: Arc<dyn Fn(usize) -> (StatusCode, String) + Send + Sync>,
) -> (String, String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let models_calls = Arc::new(AtomicUsize::new(0));
    let chat_calls = Arc::new(AtomicUsize::new(0));
    let (mc, cc) = (models_calls.clone(), chat_calls.clone());
    let (m, c) = (models.clone(), chat.clone());
    let app = Router::new()
        .route(
            "/models",
            get(move |_headers: axum::http::HeaderMap| {
                let mc = mc.clone();
                let m = m.clone();
                async move {
                    let n = mc.fetch_add(1, Ordering::SeqCst);
                    let (status, body) = m(n);
                    (status, body).into_response()
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |_headers: axum::http::HeaderMap, _body: String| {
                let cc = cc.clone();
                let c = c.clone();
                async move {
                    let n = cc.fetch_add(1, Ordering::SeqCst);
                    let (status, body) = c(n);
                    (status, body).into_response()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");
    let endpoint = format!("{base}/v1/chat/completions");
    (endpoint, base, models_calls, chat_calls)
}

/// opencode-zen rig: 1 key + 1 proxy egress "dev" dialing the mock base through
/// a real reqwest proxy client (C6: probe AND chat must go through the SAME
/// exit). Returns (key_pool, egress_pool, executor, proxy_client).
fn selfheal_rig(
    base: String,
) -> (Arc<KeyPool>, Arc<EgressPool>, UpstreamExecutor, reqwest::Client) {
    let pool = Arc::new(KeyPool::new("opencode-zen", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new("zen-1", "sk-zen-1", 1, 10));
    let eg = Arc::new(EgressPool::new("opencode-zen", EgressStrategy::RoundRobin));
    eg.add_egress(EgressEntry::proxy("dev", base.clone()));
    let proxy_client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(base.clone()).expect("proxy url must parse"))
        .build()
        .expect("proxy client must build");
    let clients = HashMap::from([(base, proxy_client.clone())]);
    let ex = UpstreamExecutor::new(pool.clone(), 3).with_egress(Some(eg.clone()), Some(clients));
    (pool, eg, ex, proxy_client)
}

fn chat_payload() -> Value {
    json!({
        "model": "space-bunny-free",
        "messages": [{"role": "user", "content": "hello"}]
    })
}

fn completion_body(msg: &str) -> String {
    json!({
        "id": "chatcmpl-selfheal",
        "object": "chat.completion",
        "created": 1710000000,
        "model": "space-bunny-free",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": msg},
            "finish_reason": "stop"
        }]
    })
    .to_string()
}

/// Cool the single "dev" exit with a long quota-429 cooldown (C1 trigger
/// state: full-pool cooldown).
fn cool_all(eg: &EgressPool) {
    eg.record_quota_exhausted("dev", Some(Duration::from_secs(900)));
}

// ---------------------------------------------------------------------------
// T1 — all-cooled + probe 200 → unlock, select again, chat succeeds
// ---------------------------------------------------------------------------

/// T1: full cooldown + recovery probe 200 → `recovery_probe_candidate` yields
/// the exit, `record_probe_success` clears the cooldown, `select_egress`
/// selects again, and the request succeeds through the unlocked exit (upstream
/// probe call >= 1 observable on /models, chat reaches the mock).
/// RED on current code: candidate/probe-success are unimplemented!() stubs.
#[tokio::test]
async fn egress_cooldown_selfheal_t1_probe_200_unlocks_egress_and_chat_succeeds() {
    // Arrange
    let ok = Arc::new(|_n: usize| {
        (StatusCode::OK, completion_body("Self-healed via probe!"))
    });
    let (endpoint, base, models_calls, chat_calls) =
        spawn_selfheal_mock(ok.clone(), ok.clone()).await;
    let (_pool, eg, ex, proxy_client) = selfheal_rig(base.clone());
    cool_all(&eg);
    assert!(eg.all_cooling(), "precondition: full cooldown required");

    // Act — recovery probe (executor's probe role driven against the frozen
    // API; C1 trigger + C6 same-exit dialing).
    eg
        .recovery_probe_candidate()
        .expect("all-cooled pool must yield a recovery-probe candidate"); // RED: unimplemented!()
    let probe = proxy_client
        .get(format!("{base}/models"))
        .send()
        .await
        .unwrap();
    assert!(
        probe.status().is_success(),
        "probe must get 200 from the mock, got {}",
        probe.status()
    );
    assert!(
        models_calls.load(Ordering::SeqCst) >= 1,
        "upstream probe call must be observable (models_calls >= 1), got {}",
        models_calls.load(Ordering::SeqCst)
    );

    // Act — C2: success record clears the cooldown.
    eg.record_probe_success("dev"); // RED: unimplemented!()
    assert!(
        matches!(eg.select_egress(), Ok(_)),
        "egress must be selectable again after probe success"
    );
    assert!(
        eg.egress_cooldown("dev").0.is_none(),
        "cooldown must be cleared by record_probe_success, got {:?}",
        eg.egress_cooldown("dev").0
    );

    // Assert — request succeeds through the unlocked exit (upstream call >= 1).
    let resp = ex
        .execute_json_request(&endpoint, &chat_payload())
        .await
        .expect("request must succeed after probe unlock");
    assert_eq!(
        resp["choices"][0]["message"]["content"],
        "Self-healed via probe!",
        "chat must reach the upstream mock, got: {resp}"
    );
    assert!(
        chat_calls.load(Ordering::SeqCst) >= 1,
        "chat must perform a real upstream call, got {}",
        chat_calls.load(Ordering::SeqCst)
    );
}

// ---------------------------------------------------------------------------
// T2 — all-cooled + probe 429 → cooldown kept, fail-fast, 0 chat calls
// ---------------------------------------------------------------------------

/// T2: full cooldown + recovery probe 429 → the candidate is still offered
/// (probe allowed, not throttled), the cooldown stays, `select_egress` keeps
/// returning Err(NoAvailableKey) and the executor fail-fasts with 0 real
/// upstream chat calls (C3).
/// RED on current code: candidate is an unimplemented!() stub.
#[tokio::test]
async fn egress_cooldown_selfheal_t2_probe_429_keeps_cooldown_and_fails_fast() {
    // Arrange
    let models = Arc::new(|_n: usize| {
        (StatusCode::TOO_MANY_REQUESTS, ZEN_FREE_USAGE_LIMIT_429_BODY.to_string())
    });
    let chat = Arc::new(|_n: usize| {
        (StatusCode::OK, completion_body("must never be reached"))
    });
    let (endpoint, base, models_calls, chat_calls) =
        spawn_selfheal_mock(models, chat).await;
    let (_pool, eg, ex, proxy_client) = selfheal_rig(base.clone());
    cool_all(&eg);
    assert!(eg.all_cooling(), "precondition: full cooldown required");

    // Act — probe allowed (not throttled), upstream answers 429.
    eg
        .recovery_probe_candidate()
        .expect("probe must be allowed (fresh, not throttled)"); // RED: unimplemented!()
    let probe = proxy_client
        .get(format!("{base}/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        probe.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "probe must get 429 from the mock, got {}",
        probe.status()
    );
    assert!(
        models_calls.load(Ordering::SeqCst) >= 1,
        "the probe attempt must be observable, got {}",
        models_calls.load(Ordering::SeqCst)
    );

    // Assert — no success record → cooldown persists (C3).
    assert!(
        matches!(eg.select_egress(), Err(CoreError::NoAvailableKey(_))),
        "cooldown must persist after a failed probe"
    );

    // Assert — executor fail-fast, 0 real upstream chat calls.
    let res = ex.execute_json_request(&endpoint, &chat_payload()).await;
    assert!(res.is_err(), "all-cooled pool must fail fast, got {res:?}");
    assert_eq!(
        chat_calls.load(Ordering::SeqCst),
        0,
        "no real chat call may reach upstream after a failed probe"
    );
}

// ---------------------------------------------------------------------------
// T3 — probe throttle: >= PROBE_INTERVAL between probes of one exit
// ---------------------------------------------------------------------------

/// T3: after a probe is recorded for an exit, `probe_throttle_remaining`
/// reports Some(<= PROBE_INTERVAL) and an immediate second
/// `recovery_probe_candidate` is suppressed (C4, 60s default).
/// RED on current code: all three probe APIs are unimplemented!() stubs.
#[tokio::test]
async fn egress_cooldown_selfheal_t3_probe_throttled_within_interval_no_second_probe() {
    // Arrange
    let ok = Arc::new(|_n: usize| (StatusCode::OK, completion_body("throttled")));
    let (endpoint, base, models_calls, _chat_calls) =
        spawn_selfheal_mock(ok.clone(), ok.clone()).await;
    let (_pool, eg, _ex, proxy_client) = selfheal_rig(base.clone());
    cool_all(&eg);

    // Act — first probe allowed and driven to success.
    let _first = eg
        .recovery_probe_candidate()
        .expect("first probe must be allowed"); // RED: unimplemented!()
    let probe = proxy_client
        .get(format!("{base}/models"))
        .send()
        .await
        .unwrap();
    assert!(probe.status().is_success(), "first probe must get 200");
    eg.record_probe_success("dev"); // RED: unimplemented!()
    assert!(
        models_calls.load(Ordering::SeqCst) >= 1,
        "the first probe must be observable, got {}",
        models_calls.load(Ordering::SeqCst)
    );

    // Assert — throttle engaged: Some(<= PROBE_INTERVAL) remaining.
    let remaining = eg.probe_throttle_remaining("dev"); // RED: unimplemented!()
    assert!(
        remaining.is_some(),
        "a just-recorded probe must leave a throttle window open"
    );
    let r = remaining.unwrap();
    assert!(
        r <= PROBE_INTERVAL,
        "throttle window must not exceed PROBE_INTERVAL ({PROBE_INTERVAL:?}), got {r:?}"
    );
    assert!(
        r > Duration::ZERO,
        "throttle window must still be open right after the probe, got {r:?}"
    );

    // Assert — C4: immediate second probe suppressed.
    let second = eg.recovery_probe_candidate(); // RED: unimplemented!()
    assert!(
        second.is_none(),
        "a throttled exit must not yield a second probe candidate"
    );

    // The chat surface is unused here — keep the endpoint binding honest.
    let _ = &endpoint;
}

// ---------------------------------------------------------------------------
// T4 — partial cooldown → no probe, healthy exit serves (C5)
// ---------------------------------------------------------------------------

/// T4: one exit cooled + one healthy → `recovery_probe_candidate` is None (no
/// probe while a healthy exit exists) and the healthy exit keeps serving
/// requests (status quo preserved).
/// RED on current code: candidate is an unimplemented!() stub.
#[tokio::test]
async fn egress_cooldown_selfheal_t4_partial_cooling_healthy_egress_serves_without_probe() {
    // Arrange — two proxy exits over the same mock base; only "dev-a" cooled.
    let ok = Arc::new(|_n: usize| {
        (StatusCode::OK, completion_body("Healthy exit still serves!"))
    });
    let (endpoint, base, models_calls, chat_calls) =
        spawn_selfheal_mock(ok.clone(), ok.clone()).await;
    let pool = Arc::new(KeyPool::new("opencode-zen", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new("zen-1", "sk-zen-1", 1, 10));
    let eg = Arc::new(EgressPool::new("opencode-zen", EgressStrategy::RoundRobin));
    eg.add_egress(EgressEntry::proxy("dev-a", base.clone()));
    eg.add_egress(EgressEntry::proxy("dev-b", base.clone()));
    let proxy_client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(base.clone()).expect("proxy url must parse"))
        .build()
        .expect("proxy client must build");
    let clients = HashMap::from([(base.clone(), proxy_client)]);
    let ex = UpstreamExecutor::new(pool.clone(), 3).with_egress(Some(eg.clone()), Some(clients));

    eg.record_quota_exhausted("dev-a", Some(Duration::from_secs(900)));
    assert!(
        !eg.all_cooling(),
        "precondition: at least one healthy exit must remain"
    );

    // Act — C5: healthy exit present → no recovery probe.
    let cand = eg.recovery_probe_candidate(); // RED: unimplemented!()
    assert!(
        cand.is_none(),
        "must not probe while a healthy exit exists, got {cand:?}"
    );
    assert_eq!(
        models_calls.load(Ordering::SeqCst),
        0,
        "no probe traffic may be issued while a healthy exit exists"
    );

    // Assert — healthy exit still serves requests.
    let resp = ex
        .execute_json_request(&endpoint, &chat_payload())
        .await
        .expect("healthy exit must serve the request");
    assert_eq!(
        resp["choices"][0]["message"]["content"],
        "Healthy exit still serves!",
        "got: {resp}"
    );
    assert!(
        chat_calls.load(Ordering::SeqCst) >= 1,
        "the healthy exit must reach upstream, got {}",
        chat_calls.load(Ordering::SeqCst)
    );
}

// ---------------------------------------------------------------------------
// T5 — empty pool → NoAvailableKey, no probe (C5)
// ---------------------------------------------------------------------------

/// T5: empty egress pool → `recovery_probe_candidate` is None and
/// `select_egress` fails with NoAvailableKey; no probe traffic.
/// RED on current code: candidate is an unimplemented!() stub.
#[tokio::test]
async fn egress_cooldown_selfheal_t5_empty_pool_no_candidate_no_probe() {
    // Arrange
    let ok = Arc::new(|_n: usize| (StatusCode::OK, completion_body("unused")));
    let (_endpoint, base, models_calls, _chat_calls) =
        spawn_selfheal_mock(ok.clone(), ok.clone()).await;
    let eg = Arc::new(EgressPool::new("opencode-zen", EgressStrategy::RoundRobin));
    assert!(eg.is_empty(), "precondition: empty pool");

    // Act + Assert — C5: empty pool → no candidate, no probe, NoAvailableKey.
    let cand = eg.recovery_probe_candidate(); // RED: unimplemented!()
    assert!(
        cand.is_none(),
        "empty pool must not yield a probe candidate, got {cand:?}"
    );
    assert!(
        matches!(eg.select_egress(), Err(CoreError::NoAvailableKey(_))),
        "empty pool must select-fail with NoAvailableKey"
    );
    assert_eq!(
        models_calls.load(Ordering::SeqCst),
        0,
        "no probe traffic may be issued for an empty pool"
    );
    let _ = &base;
}

// ---------------------------------------------------------------------------
// T6 — stream twin (C8): probe 200 unlock + probe 429 fail-fast on the stream
// executor path. `execute_stream_request_with_timing_and_key` returns the raw
// response (upstream.rs:2502), so the mock serves status-level bodies; no SSE
// payload is required and the twin mirrors T1/T2 exactly.
// ---------------------------------------------------------------------------

/// T6-1 (stream twin of T1): probe 200 → unlock → the stream path succeeds
/// through the unlocked exit (status 200, winning key zen-1, upstream call).
/// RED on current code: candidate/probe-success are unimplemented!() stubs.
#[tokio::test]
async fn egress_cooldown_selfheal_t6a_stream_probe_200_unlocks_and_stream_path_succeeds() {
    // Arrange
    let ok = Arc::new(|_n: usize| (StatusCode::OK, completion_body("Self-healed via probe!")));
    let (endpoint, base, models_calls, chat_calls) =
        spawn_selfheal_mock(ok.clone(), ok.clone()).await;
    let (_pool, eg, ex, proxy_client) = selfheal_rig(base.clone());
    cool_all(&eg);
    assert!(eg.all_cooling(), "precondition: full cooldown required");

    // Act — recovery probe 200 (same path as T1).
    eg
        .recovery_probe_candidate()
        .expect("all-cooled pool must yield a recovery-probe candidate"); // RED: unimplemented!()
    let probe = proxy_client
        .get(format!("{base}/models"))
        .send()
        .await
        .unwrap();
    assert!(probe.status().is_success(), "probe must get 200");
    assert!(models_calls.load(Ordering::SeqCst) >= 1);
    eg.record_probe_success("dev"); // RED: unimplemented!()
    assert!(matches!(eg.select_egress(), Ok(_)), "egress must be selectable again");

    // Assert — stream executor path succeeds through the unlocked exit.
    let (resp, _instant, key_id) = ex
        .execute_stream_request_with_timing_and_key(&endpoint, &chat_payload())
        .await
        .expect("stream path must succeed after probe unlock");
    assert!(
        resp.status().is_success(),
        "stream upstream response must be 200, got {}",
        resp.status()
    );
    assert_eq!(key_id, "zen-1", "winning key must be zen-1, got {key_id}");
    assert!(
        chat_calls.load(Ordering::SeqCst) >= 1,
        "stream path must perform a real upstream call, got {}",
        chat_calls.load(Ordering::SeqCst)
    );
}

/// T6-2 (stream twin of T2): probe 429 → cooldown kept → the stream path
/// fail-fasts with 0 upstream chat calls (C8 symmetry with the JSON path).
/// RED on current code: candidate is an unimplemented!() stub.
#[tokio::test]
async fn egress_cooldown_selfheal_t6b_stream_probe_429_keeps_cooldown_and_stream_fails_fast() {
    // Arrange
    let models = Arc::new(|_n: usize| {
        (StatusCode::TOO_MANY_REQUESTS, ZEN_FREE_USAGE_LIMIT_429_BODY.to_string())
    });
    let chat = Arc::new(|_n: usize| (StatusCode::OK, completion_body("must never be reached")));
    let (endpoint, base, models_calls, chat_calls) =
        spawn_selfheal_mock(models, chat).await;
    let (_pool, eg, ex, proxy_client) = selfheal_rig(base.clone());
    cool_all(&eg);
    assert!(eg.all_cooling(), "precondition: full cooldown required");

    // Act — probe allowed, upstream answers 429.
    eg
        .recovery_probe_candidate()
        .expect("probe must be allowed (fresh, not throttled)"); // RED: unimplemented!()
    let probe = proxy_client
        .get(format!("{base}/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(probe.status(), StatusCode::TOO_MANY_REQUESTS, "probe must get 429");
    assert!(models_calls.load(Ordering::SeqCst) >= 1);
    assert!(
        matches!(eg.select_egress(), Err(CoreError::NoAvailableKey(_))),
        "cooldown must persist after a failed probe"
    );

    // Assert — stream path fail-fasts with 0 upstream chat calls (C8).
    let res = ex
        .execute_stream_request_with_timing_and_key(&endpoint, &chat_payload())
        .await;
    assert!(res.is_err(), "stream path must fail fast on all-cooled pool, got {res:?}");
    assert_eq!(
        chat_calls.load(Ordering::SeqCst),
        0,
        "stream path must not reach upstream after a failed probe"
    );
}

// ---------------------------------------------------------------------------
// T7 (L3-B review) — executor INLINE recovery probe, full chain
// ---------------------------------------------------------------------------

/// T7: the EXECUTOR's inline recovery probe (upstream.rs:1385
/// `egress_recovery_probe`). An all-cooled `execute_json_request` must itself
/// probe `GET {base}/models` (200 valid JSON), unlock the exit via
/// `record_probe_success`, reselect it and POST the chat to success — no
/// manual API driving. GREEN on the landed impl; any regression that drops
/// the inline probe re-fails this case.
#[tokio::test]
async fn egress_cooldown_selfheal_t7_inline_probe_200_unlocks_and_chat_succeeds() {
    // Arrange
    let models = Arc::new(|_n: usize| {
        (StatusCode::OK, r#"{"object":"list","data":[]}"#.to_string())
    });
    let chat = Arc::new(|_n: usize| {
        (StatusCode::OK, completion_body("Inline self-heal success!"))
    });
    let (endpoint, base, models_calls, chat_calls) =
        spawn_selfheal_mock(models, chat).await;
    let (_pool, eg, ex, _proxy_client) = selfheal_rig(base.clone());
    cool_all(&eg);
    assert!(eg.all_cooling(), "precondition: full cooldown required");

    // Act — the executor's inline probe fires inside the request path.
    let resp = ex
        .execute_json_request(&endpoint, &chat_payload())
        .await
        .expect("inline probe 200 must unlock and let the request succeed");

    // Assert — probe fired, unlock persisted, chat succeeded (full chain).
    assert!(
        models_calls.load(Ordering::SeqCst) >= 1,
        "the inline recovery probe must hit GET /models, got {}",
        models_calls.load(Ordering::SeqCst)
    );
    assert_eq!(
        resp["choices"][0]["message"]["content"],
        "Inline self-heal success!",
        "chat must succeed through the unlocked exit, got: {resp}"
    );
    assert!(
        chat_calls.load(Ordering::SeqCst) >= 1,
        "a real upstream chat call must occur, got {}",
        chat_calls.load(Ordering::SeqCst)
    );
    assert!(
        eg.egress_cooldown("dev").0.is_none(),
        "the inline probe success must clear the cooldown, got {:?}",
        eg.egress_cooldown("dev").0
    );
    assert!(
        matches!(eg.select_egress(), Ok(_)),
        "the exit must be reselectable after the inline probe unlock"
    );
}
