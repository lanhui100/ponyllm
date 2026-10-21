//! Wave-1 egress self-heal L2-AT adversarial suite (contract
//! `.dev-team/contracts/2026-10-22-egress-cooldown-selfheal.md`, frozen by Lead;
//! task-2, Test Agent). Frozen path `tests/acceptance/adversarial/egress_selfheal/`.
//!
//! This file is wired into the ponyllm-core integration target via
//! `#[path]` from `crates/ponyllm-core/tests/egress_cooldown_selfheal_tests.rs`
//! so it is machine-runnable: `cargo test -p ponyllm-core egress_cooldown_selfheal`.
//! It is fully self-contained (its own mock / rig / fixtures).
//!
//! Coverage:
//!   - A1: probe injected timeout (>3s NFR hard limit) → no panic, cooldown
//!     kept, executor fail-fast inside a tokio::time::timeout (no hang).
//!   - A2: malformed probe (200 + empty non-JSON body) → frozen as a probe
//!     failure: cooldown kept, fail-fast, 0 upstream chat calls.
//!     [RED DEGRADED: BODY-AGNOSTIC] — if the impl accepts any 200 as probe
//!     success, A2's cooldown-kept / chat_calls==0 asserts flip; the Executor
//!     must then add body validation per A2 or obtain Lead approval for the
//!     degradation (the no-panic part of A2 still holds).
//!   - A3: 4 concurrent all-cooled probe requests (barrier-aligned) → exactly
//!     one probe wins (throttle atomicity, C4): candidate-Some count == 1 and
//!     models_calls == 1.
//!
//! RED PHASE: `recovery_probe_candidate` / `record_probe_success` /
//! `probe_throttle_remaining` are `unimplemented!()` stubs (egress.rs
//! L353/L361/L368) — every case below panics on the current code (red).

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{routing::{get, post}, Router};
use ponyllm_core::error::CoreError;
use ponyllm_core::executor::*;
use ponyllm_core::pool::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Local axum mock with GET /models + POST /v1/chat/completions over sync
/// handlers (A2/A3 use this).
async fn spawn_l2at_mock(
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

/// A1-only mock: GET /models sleeps 3.5s (beyond the 3s NFR hard probe
/// timeout) then answers 200; chat answers 200.
async fn spawn_slow_probe_mock() -> (String, String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let models_calls = Arc::new(AtomicUsize::new(0));
    let chat_calls = Arc::new(AtomicUsize::new(0));
    let (mc, cc) = (models_calls.clone(), chat_calls.clone());
    let app = Router::new()
        .route(
            "/models",
            get(move |_headers: axum::http::HeaderMap| {
                let mc = mc.clone();
                async move {
                    mc.fetch_add(1, Ordering::SeqCst);
                    // > 3000ms NFR hard probe timeout (contract NFR baseline).
                    tokio::time::sleep(Duration::from_millis(3500)).await;
                    (StatusCode::OK, "{}".to_string()).into_response()
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |_headers: axum::http::HeaderMap, _body: String| {
                let cc = cc.clone();
                async move {
                    cc.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, completion_body("must never be reached")).into_response()
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

/// 1 key + 1 proxy egress "dev" over the mock base (same-exit C6 wiring).
fn adversarial_rig(
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

fn cool_all(eg: &EgressPool) {
    eg.record_quota_exhausted("dev", Some(Duration::from_secs(900)));
}

// ---------------------------------------------------------------------------
// A1 — probe injected timeout (>3s) → no panic, cooldown kept, fail-fast
// ---------------------------------------------------------------------------

/// A1: the /models handler sleeps 3.5s (> 3s NFR hard timeout). Frozen
/// contract: the probe times out, the cooldown is kept, the executor
/// fail-fasts without hanging and without a real upstream chat call.
///
/// Assertions:
///   - candidate is offered (probe allowed, not throttled);
///   - the probe GET times out client-side at 3.1s vs the 3.5s server sleep
///     (the 3s hard-timeout contract is honored at the wire level);
///   - no panic anywhere; cooldown stays; the executor call completes inside a
///     tokio::time::timeout(5s) with Err and chat_calls == 0.
/// RED on current code: candidate panics (unimplemented!()).
#[tokio::test]
async fn egress_cooldown_selfheal_a1_probe_timeout_keeps_cooldown_and_fails_fast() {
    // Arrange
    let (endpoint, base, models_calls, chat_calls) = spawn_slow_probe_mock().await;
    let (_pool, eg, ex, proxy_client) = adversarial_rig(base.clone());
    cool_all(&eg);
    assert!(eg.all_cooling(), "precondition: full cooldown required");

    // Act — probe drive: candidate allowed, GET must time out (server sleeps
    // 3.5s > the 3.1s client window, i.e. beyond the 3s NFR hard limit).
    eg
        .recovery_probe_candidate()
        .expect("probe must be allowed (fresh, not throttled)"); // RED: unimplemented!()
    let probe = tokio::time::timeout(
        Duration::from_millis(3100),
        proxy_client.get(format!("{base}/models")).send(),
    )
    .await;
    assert!(
        probe.is_err(),
        "probe must time out (server sleeps 3.5s > 3s NFR), got {probe:?}"
    );
    assert!(
        models_calls.load(Ordering::SeqCst) >= 1,
        "the probe attempt must be observable, got {}",
        models_calls.load(Ordering::SeqCst)
    );

    // Assert — cooldown kept (no success record), no panic, fail-fast.
    assert!(
        matches!(eg.select_egress(), Err(CoreError::NoAvailableKey(_))),
        "cooldown must persist after a timed-out probe"
    );
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        ex.execute_json_request(&endpoint, &chat_payload()),
    )
    .await;
    assert!(
        res.is_ok(),
        "executor must fail fast without hanging past the outer timeout, got {res:?}"
    );
    let inner = res.unwrap();
    assert!(inner.is_err(), "all-cooled pool must fail fast, got {inner:?}");
    assert_eq!(
        chat_calls.load(Ordering::SeqCst),
        0,
        "no upstream chat call may reach the mock after a timed-out probe"
    );
}

// ---------------------------------------------------------------------------
// A2 — malformed probe (200 + empty non-JSON body) → probe failure
// ---------------------------------------------------------------------------

/// A2: /models answers 200 with an EMPTY body (non-JSON). Frozen contract:
/// a malformed probe is a probe FAILURE → cooldown kept → executor fail-fast,
/// 0 upstream chat calls.
///
/// [RED DEGRADED: BODY-AGNOSTIC] — if the impl treats any 200 as probe
/// success without body validation, the cooldown-kept and chat_calls==0
/// asserts below flip (the executor would unlock and chat once); the Executor
/// must then add body validation per A2 or obtain Lead approval for the
/// degradation. The no-panic property of A2 holds either way.
/// RED on current code: candidate panics (unimplemented!()).
#[tokio::test]
async fn egress_cooldown_selfheal_a2_malformed_empty_body_probe_keeps_cooldown_and_fails_fast() {
    // Arrange — 200 with an empty, non-JSON body on /models.
    let models = Arc::new(|_n: usize| (StatusCode::OK, String::new()));
    let chat = Arc::new(|_n: usize| (StatusCode::OK, completion_body("must never be reached")));
    let (endpoint, base, models_calls, chat_calls) = spawn_l2at_mock(models, chat).await;
    let (_pool, eg, ex, proxy_client) = adversarial_rig(base.clone());
    cool_all(&eg);
    assert!(eg.all_cooling(), "precondition: full cooldown required");

    // Act — probe drive: 200 but empty (non-JSON) body.
    eg
        .recovery_probe_candidate()
        .expect("probe must be allowed (fresh, not throttled)"); // RED: unimplemented!()
    let probe = proxy_client
        .get(format!("{base}/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(probe.status(), StatusCode::OK, "wire precondition: 200");
    let body = probe.text().await.unwrap();
    assert!(body.trim().is_empty(), "wire precondition: empty body");
    assert!(models_calls.load(Ordering::SeqCst) >= 1);

    // Assert — frozen: malformed body = probe failure → cooldown kept,
    // fail-fast, 0 upstream chat calls.
    assert!(
        matches!(eg.select_egress(), Err(CoreError::NoAvailableKey(_))),
        "cooldown must be kept after a malformed probe body"
    );
    let res = ex.execute_json_request(&endpoint, &chat_payload()).await;
    assert!(res.is_err(), "fail-fast must hold after a malformed probe, got {res:?}");
    assert_eq!(
        chat_calls.load(Ordering::SeqCst),
        0,
        "0 upstream chat calls after a malformed probe (flips only under BODY-AGNOSTIC)"
    );
}

// ---------------------------------------------------------------------------
// A3 — 4 concurrent all-cooled probes → exactly ONE probe (throttle atomicity)
// ---------------------------------------------------------------------------

/// A3: 4 tasks hit `recovery_probe_candidate` concurrently on the same
/// all-cooled single exit, barrier-aligned for a deterministic race. Frozen
/// contract (C4): the throttle is atomic — exactly one probe wins; the winner
/// performs the single GET /models (models_calls == 1) and the throttle stays
/// engaged afterwards.
/// RED on current code: the stub panics inside the spawned tasks → join fails.
#[tokio::test]
async fn egress_cooldown_selfheal_a3_concurrent_all_cooled_probe_runs_exactly_once() {
    // Arrange
    let models = Arc::new(|_n: usize| (StatusCode::OK, "{}".to_string()));
    let chat = Arc::new(|_n: usize| (StatusCode::OK, completion_body("concurrent")));
    let (_endpoint, base, models_calls, _chat_calls) = spawn_l2at_mock(models, chat).await;
    let (_pool, eg, _ex, proxy_client) = adversarial_rig(base.clone());
    cool_all(&eg);
    assert!(eg.all_cooling(), "precondition: full cooldown required");

    // Act — 4 barrier-aligned concurrent candidate requests.
    let barrier = Arc::new(tokio::sync::Barrier::new(4));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let barrier = barrier.clone();
        let eg = eg.clone();
        let proxy_client = proxy_client.clone();
        let base = base.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await; // deterministic race alignment, no blind sleep
            let cand = eg.recovery_probe_candidate(); // RED: unimplemented!() panics in-task
            match cand {
                Some(_) => {
                    // The single allowed probe: GET /models through the same exit.
                    let probe = proxy_client
                        .get(format!("{base}/models"))
                        .send()
                        .await
                        .map(|r| r.status());
                    (1usize, probe.ok())
                }
                None => (0usize, None),
            }
        }));
    }
    let mut winners = 0usize;
    let mut probe_statuses = Vec::new();
    for h in handles {
        let (w, st) = h.await.unwrap_or_else(|e| panic!("task must not panic: {e}"));
        winners += w;
        if let Some(s) = st {
            probe_statuses.push(s);
        }
    }

    // Assert — exactly one probe won and exactly one GET reached upstream.
    assert_eq!(
        winners, 1,
        "exactly one concurrent probe must win (throttle atomicity), got {winners}"
    );
    assert_eq!(
        probe_statuses.len(),
        1,
        "exactly one probe GET must reach upstream, got {}",
        probe_statuses.len()
    );
    assert!(
        probe_statuses[0].is_success(),
        "the winning probe must get 200, got {}",
        probe_statuses[0]
    );
    assert_eq!(
        models_calls.load(Ordering::SeqCst),
        1,
        "probe counter must be exactly 1, got {}",
        models_calls.load(Ordering::SeqCst)
    );
    // Throttle stays engaged after the winning probe.
    assert!(
        eg.probe_throttle_remaining("dev").is_some(), // RED: unimplemented!()
        "throttle must remain engaged after the winning probe"
    );
}
