//! Red-phase acceptance tests — CONTRACT-v1 C2 (zen upstream header set),
//! observed on the wire against a **loopback** axum mock (127.0.0.1:0).
//!
//! Frozen contract: `/home/dm/ponyllm/.dev-team/zen-rc/CONTRACT-v1.md` (read-only).
//!
//! Rationale for the loopback route: `UpstreamExecutor::build_headers` is a
//! private inherent method, so an external integration test cannot call it.
//! This file therefore drives one real upstream attempt per observation into a
//! mock bound to 127.0.0.1:0 — no external network, no production API widening.
//!
//! C2 requires, for `opencode_zen == true`:
//!   x-opencode-session / x-opencode-session-id / x-session-affinity /
//!   x-session-id / X-Session-Id  == session
//!                                  (`^ses_[0-9a-f]{12}[0-9A-Za-z]{14}$`),
//!   x-opencode-project      == `^prj_[0-9a-f]{32}$`   (process-stable),
//!   x-opencode-request-id   == `^req_[0-9a-f]{32}$`   (fresh per attempt),
//!   User-Agent              == `opencode/1.18.35 (Linux; x64)`.
//! Outside the zen scope no `x-opencode-*` / `x-session-*` header may appear.
//!
//! Red-phase discipline: real assertions only, no `#[ignore]`, no empty bodies.

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{routing::post, Router};
use ponyllm_core::executor::UpstreamExecutor;
use ponyllm_core::pool::{ApiKeyEntry, KeyPool, RoutingStrategy};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Minimal valid SSE frame for the zen upstream stream wire.
const SSE_FRAME: &str = "data: {\"id\":\"chatcmpl-c2\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\ndata: [DONE]\n\n";

type CapturedHeaders = Arc<Mutex<Vec<HeaderMap>>>;

/// Loopback mock: records every received header map, answers 200 + SSE.
async fn spawn_header_mock() -> (String, CapturedHeaders) {
    let captured: CapturedHeaders = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();
    let app = Router::new().route(
        "/zen/v1/chat/completions",
        post(move |headers: HeaderMap, _body: String| {
            let cap = cap.clone();
            async move {
                cap.lock().unwrap().push(headers);
                (
                    StatusCode::OK,
                    [("content-type", "text/event-stream")],
                    SSE_FRAME,
                )
                    .into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        format!("http://{addr}/zen/v1/chat/completions"),
        captured,
    )
}

fn zen_executor() -> UpstreamExecutor {
    let pool = Arc::new(KeyPool::new("opencode-zen", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    UpstreamExecutor::new(pool, 1)
        .with_downstream_headers(&HeaderMap::new())
        .with_opencode_zen(true)
}

fn zen_payload() -> Value {
    json!({
        "model": "deepseek-v4.1-flash-free",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    })
}

/// Contract C1/C2 session shape: `^ses_[0-9a-f]{12}[0-9A-Za-z]{14}$` — 12
/// lowercase-hex chars (the upstream free-tier gate's load-bearing prefix)
/// followed by 14 base62 chars. Kept in lockstep with
/// `ponyllm_core::executor::is_valid_zen_session_format`.
fn is_contract_session(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("ses_") else {
        return false;
    };
    let bytes = rest.as_bytes();
    bytes.len() == 26
        && bytes[..12]
            .iter()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        && bytes[12..].iter().all(|b| b.is_ascii_alphanumeric())
}

/// `prefix` + exactly 32 lowercase hex chars (`prj_` / `req_`).
fn is_prefixed_lower_hex(value: &str, prefix: &str) -> bool {
    match value.strip_prefix(prefix) {
        Some(rest) => rest.len() == 32 && rest.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        None => false,
    }
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn observed(headers: &HeaderMap) -> Vec<&str> {
    headers.keys().map(|k| k.as_str()).collect()
}

/// Asserts the full frozen C2 table on one wire-observed header map.
fn assert_fingerprint_headers(headers: &HeaderMap, label: &str) {
    let session = header_str(headers, "x-opencode-session").unwrap_or_else(|| {
        panic!(
            "CONTRACT C2: {label}: missing `x-opencode-session`; observed headers: {:?}",
            observed(headers)
        )
    });
    assert!(
        is_contract_session(&session),
        "CONTRACT C1/C2: {label}: session must match \
         ^ses_[0-9a-f]{{12}}[0-9A-Za-z]{{14}}$, got {session:?}"
    );

    // NOTE: HTTP header names are case-insensitive and hyper lowercases them on
    // the wire, so the mock cannot distinguish the contract's `x-session-id`
    // and `X-Session-Id` rows — one lowercase name carrying the session value
    // satisfies both.
    for name in [
        "x-opencode-session",
        "x-opencode-session-id",
        "x-session-affinity",
        "x-session-id",
        "X-Session-Id",
    ] {
        let got = header_str(headers, name);
        assert_eq!(
            got.as_deref(),
            Some(session.as_str()),
            "CONTRACT C2: {label}: `{name}` must be present and equal to the session \
             `{session}`; got {got:?}; observed headers: {:?}",
            observed(headers)
        );
    }

    let project = header_str(headers, "x-opencode-project").unwrap_or_default();
    assert!(
        is_prefixed_lower_hex(&project, "prj_"),
        "CONTRACT C2: {label}: `x-opencode-project` must match ^prj_[0-9a-f]{{32}}$, \
         got {project:?}; observed headers: {:?}",
        observed(headers)
    );

    let request_id = header_str(headers, "x-opencode-request-id").unwrap_or_default();
    assert!(
        is_prefixed_lower_hex(&request_id, "req_"),
        "CONTRACT C2: {label}: `x-opencode-request-id` must match ^req_[0-9a-f]{{32}}$, \
         got {request_id:?}; observed headers: {:?}",
        observed(headers)
    );

    assert_eq!(
        header_str(headers, "user-agent").as_deref(),
        Some("opencode/1.18.35 (Linux; x64)"),
        "CONTRACT C2: {label}: User-Agent must be `opencode/1.18.35 (Linux; x64)`, \
         got {:?}; observed headers: {:?}",
        header_str(headers, "user-agent"),
        observed(headers)
    );
}

/// C2 main: full fingerprint set on the wire, project stable, request-id fresh.
#[tokio::test]
async fn c2_zen_wire_headers_carry_full_fingerprint_set() {
    // C3 pacing is covered by its own test file; keep these C2 observations fast.
    std::env::set_var("PONYLLM_ZEN_MIN_INTERVAL_MS", "0");

    let (endpoint, captured) = spawn_header_mock().await;
    let executor = zen_executor();
    let body = zen_payload();

    let _first = executor
        .execute_stream_request_with_timing_and_key(&endpoint, &body)
        .await
        .expect("first zen attempt must reach the mock (200 + SSE)");
    let _second = executor
        .execute_stream_request_with_timing_and_key(&endpoint, &body)
        .await
        .expect("second zen attempt must reach the mock (200 + SSE)");

    let captured = captured.lock().unwrap().clone();
    assert_eq!(
        captured.len(),
        2,
        "CONTRACT C2: expected exactly 2 observed upstream attempts, got {}",
        captured.len()
    );

    assert_fingerprint_headers(&captured[0], "attempt #1");
    assert_fingerprint_headers(&captured[1], "attempt #2");

    let req_1 = header_str(&captured[0], "x-opencode-request-id").unwrap_or_default();
    let req_2 = header_str(&captured[1], "x-opencode-request-id").unwrap_or_default();
    assert_ne!(
        req_1, req_2,
        "CONTRACT C2: `x-opencode-request-id` must be freshly generated per upstream attempt, \
         got the same value {req_1:?} on both attempts"
    );

    let project_1 = header_str(&captured[0], "x-opencode-project").unwrap_or_default();
    let project_2 = header_str(&captured[1], "x-opencode-project").unwrap_or_default();
    assert_eq!(
        project_1, project_2,
        "CONTRACT C2: `x-opencode-project` is a process-level constant and must be identical \
         across attempts, got {project_1:?} then {project_2:?}"
    );
}

/// C2 isolation regression: non-zen executor must leak no opencode/session header.
#[tokio::test]
async fn c2_non_zen_wire_headers_leak_no_opencode_or_session_headers() {
    let (endpoint, captured) = spawn_header_mock().await;
    let pool = Arc::new(KeyPool::new("sense", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let plain = UpstreamExecutor::new(pool, 1); // opencode_zen stays false

    let body = json!({
        "model": "sense-chat",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    });
    let _resp = plain
        .execute_stream_request_with_timing_and_key(&endpoint, &body)
        .await
        .expect("non-zen attempt must reach the mock (200 + SSE)");

    let captured = captured.lock().unwrap().clone();
    assert_eq!(
        captured.len(),
        1,
        "CONTRACT C2 isolation: expected exactly 1 observed non-zen attempt, got {}",
        captured.len()
    );

    let headers = &captured[0];
    let names = observed(headers);
    for name in &names {
        assert!(
            !name.starts_with("x-opencode-") && !name.starts_with("x-session-"),
            "CONTRACT C2 isolation: non-zen upstream must send no `x-opencode-*` / `x-session-*` \
             header, but saw `{name}`; observed headers: {names:?}"
        );
    }
    assert!(
        headers.get("x-opencode-client").is_none(),
        "CONTRACT C2 isolation: non-zen upstream must not send `x-opencode-client`; \
         observed headers: {names:?}"
    );
    assert!(
        headers.get("x-opencode-session-id").is_none()
            && headers.get("x-opencode-project").is_none()
            && headers.get("x-opencode-request-id").is_none(),
        "CONTRACT C2 isolation: non-zen upstream must not send \
         `x-opencode-session-id` / `x-opencode-project` / `x-opencode-request-id`; \
         observed headers: {names:?}"
    );
    assert!(
        headers.get("user-agent").is_none(),
        "CONTRACT C2 isolation: non-zen upstream must keep its historical wire shape \
         (no OpenCode User-Agent); observed headers: {names:?}"
    );
}
