//! FROZEN RED-PHASE ACCEPTANCE — Wave-2 P1 hardening (N1 requestId + M2 proxy
//! fallback). Contract (frozen by Lead, v3/v4):
//!
//! - N1: the Antigravity `requestId` header is taken from the client body ONLY
//!   when the value is length ≤ 128 AND every character ∈ `[A-Za-z0-9_.:/-]`.
//!   Anything else (CRLF, spaces, `;`/`?`, 200-char overlong) MUST fall back to
//!   the gateway-generated `agent/<uuid>/<millis>/traj-default/1` form, so a
//!   client value can never smuggle a header/log-injection payload.
//! - M2: a proxy that cannot be built (e.g. `socks5://…`: this workspace's
//!   reqwest has no `socks` feature) must fall back to a client that explicitly
//!   dials DIRECT — it must not silently inherit ambient `*_PROXY` env vars.
//!
//! Test receipts are behavioural: every case drives the real
//! `UpstreamExecutor::execute_json_request_with_key` against a local mock
//! upstream and asserts the wire-level `requestId` header it actually sent.
//! No `#[ignore]`, no smoke-only bodies, no swallowed errors.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::executor::{create_upstream_http_client_with_options, UpstreamExecutor};
use ponyllm_core::pool::{
    AntigravityCredential, AntigravityTokenManager, ApiKeyEntry, KeyPool, RoutingStrategy,
};
use serde_json::{json, Value};

/// The gateway-generated / client-accepted `requestId` character set (N1).
const REQUEST_ID_ALLOWED: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_.:/-";
const REQUEST_ID_MAX_LEN: usize = 128;

// ---------------------------------------------------------------------------
// Mock upstream that records the `requestId` header it received
// ---------------------------------------------------------------------------

async fn spawn_upstream_capture() -> (String, Arc<Mutex<Vec<String>>>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: axum::http::HeaderMap, _body: String| {
            let sink = sink.clone();
            async move {
                let request_id = headers
                    .get("requestid")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                sink.lock()
                    .expect("requestId capture lock")
                    .push(request_id.unwrap_or_else(|| "<absent>".to_string()));
                Json(json!({
                    "id": "msg_capture",
                    "object": "chat.completion",
                    "created": 0,
                    "model": "gemini-3.8-flash",
                    "choices": []
                }))
                .into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/v1/chat/completions"), seen.clone())
}

/// Antigravity key with a valid cached access token (no network refresh: the
/// token manager's fast path returns it because the expiry is in the future).
fn antigravity_executor() -> UpstreamExecutor {
    let cred = AntigravityCredential {
        access_token: Some("test-access-token".to_string()),
        refresh_token: "1//test-refresh-token".to_string(),
        client_id: "test-client-id".to_string(),
        client_secret: "test-client-secret".to_string(),
        project_id: "test-project".to_string(),
        expiry: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
    };
    let manager = Arc::new(AntigravityTokenManager::new(
        "ag-key",
        cred,
        reqwest::Client::new(),
    ));
    let pool = Arc::new(KeyPool::new("antigravity", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new_antigravity("ag-key", manager, 1, 10));
    UpstreamExecutor::new(pool, 1)
}

/// Drive one real request carrying `client_request_id` and return the header
/// value the mock upstream actually received.
async fn upstream_request_id_for(client_request_id: &str) -> String {
    let (endpoint, seen) = spawn_upstream_capture().await;
    let executor = antigravity_executor();
    let body = json!({
        "model": "gemini-3.8-flash",
        "requestId": client_request_id,
        "contents": [{"role": "user", "parts": [{"text": "hi"}]}]
    });
    executor
        .execute_json_request_with_key(&endpoint, &body)
        .await
        .expect("antigravity upstream call must succeed against the local mock");
    let captured = seen.lock().expect("capture lock").clone();
    assert_eq!(
        captured.len(),
        1,
        "N1: exactly one upstream attempt must be captured, got {captured:?}"
    );
    captured[0].clone()
}

fn assert_gateway_generated(captured: &str, client_value: &str, case: &str) {
    assert_ne!(
        captured, client_value,
        "N1[{case}]: client-supplied requestId must be replaced by the gateway value"
    );
    assert_ne!(
        captured, "<absent>",
        "N1[{case}]: the gateway must always send a requestId header, even when it rejects the client value"
    );
    assert!(
        captured.starts_with("agent/"),
        "N1[{case}]: gateway-generated requestId must use the canonical `agent/…` form, got '{captured}'"
    );
    assert!(
        captured.contains("/traj-default/"),
        "N1[{case}]: gateway-generated requestId must use the canonical traj-default segment, got '{captured}'"
    );
    assert!(
        captured.len() <= REQUEST_ID_MAX_LEN,
        "N1[{case}]: gateway-generated requestId must stay within {REQUEST_ID_MAX_LEN} chars, got {} ('{captured}')",
        captured.len()
    );
    assert!(
        captured.chars().all(|c| REQUEST_ID_ALLOWED.contains(c)),
        "N1[{case}]: gateway-generated requestId must only use the accepted charset, got '{captured}'"
    );
    assert!(
        !captured.contains('\r') && !captured.contains('\n'),
        "N1[{case}]: requestId must never carry CR/LF (header injection), got {captured:?}"
    );
}

// ---------------------------------------------------------------------------
// N1 — invalid client requestId values fall back to the gateway value
// ---------------------------------------------------------------------------

#[tokio::test]
async fn n1_crlf_request_id_falls_back_to_gateway_value() {
    let hostile = "agent/u/1/traj-1/1\r\nX-Injected: evil";
    let captured = upstream_request_id_for(hostile).await;
    assert_gateway_generated(&captured, hostile, "crlf");
}

#[tokio::test]
async fn n1_space_request_id_falls_back_to_gateway_value() {
    let hostile = "agent/u/1/traj 1/1";
    let captured = upstream_request_id_for(hostile).await;
    assert_gateway_generated(&captured, hostile, "space");
}

#[tokio::test]
async fn n1_overlong_request_id_falls_back_to_gateway_value() {
    // 200 chars of otherwise-legitimate charset: length alone must reject it.
    let hostile = "a".repeat(200);
    assert!(hostile.len() > REQUEST_ID_MAX_LEN);
    let captured = upstream_request_id_for(&hostile).await;
    assert_gateway_generated(&captured, &hostile, "overlong-200");
}

#[tokio::test]
async fn n1_illegal_char_request_id_falls_back_to_gateway_value() {
    for hostile in [
        "agent/u/1/traj-1/1;rm -rf",
        "agent/u/1/traj-1/1?x=1",
        "agent/u/1/traj|1/1",
    ] {
        let captured = upstream_request_id_for(hostile).await;
        assert_gateway_generated(&captured, hostile, "illegal-char");
    }
}

// ---------------------------------------------------------------------------
// N1 — legitimate client requestId values are preserved verbatim
// ---------------------------------------------------------------------------

#[tokio::test]
async fn n1_legal_40_char_request_id_is_preserved() {
    let legal = "agent/01HXYZABCD/1700000000000/traj-ab/1";
    assert_eq!(legal.len(), 40, "fixture must be exactly 40 chars");
    assert!(legal.chars().all(|c| REQUEST_ID_ALLOWED.contains(c)));
    let captured = upstream_request_id_for(legal).await;
    assert_eq!(
        captured, legal,
        "N1: a legal 40-char requestId must be forwarded verbatim"
    );
}

#[tokio::test]
async fn n1_canonical_agent_request_id_is_preserved() {
    let canonical = "agent/0f0e0d0c-1234-4abc-9def-0123456789ab/1700000000000/traj-default/1";
    assert!(
        canonical.starts_with("agent/")
            && canonical.ends_with("/traj-default/1")
            && canonical.len() <= REQUEST_ID_MAX_LEN,
        "fixture must be the canonical gateway shape, len = {}",
        canonical.len()
    );
    let captured = upstream_request_id_for(canonical).await;
    assert_eq!(
        captured, canonical,
        "N1: the canonical gateway shape must round-trip unchanged (body/header semantics stay aligned)"
    );
}

// ---------------------------------------------------------------------------
// M2 — a proxy that cannot be built must not fall back onto ambient proxies
// ---------------------------------------------------------------------------

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

async fn spawn_plain_ok_server() -> String {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(
            move |_headers: axum::http::HeaderMap, _body: String| async move {
                Json(json!({"ok": true})).into_response()
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/v1/chat/completions")
}

#[tokio::test]
async fn m2_unbuildable_proxy_falls_back_to_direct_dial() {
    // Trigger: a proxy string reqwest cannot build (invalid port literal). The
    // socks5 *scheme* policy is enforced one layer up in
    // `validate_egress_entry` (see crates/ponyllm-config tests); this test
    // covers the runtime fallback for ANY unbuildable proxy.
    let build_err = ponyllm_core::executor::try_create_upstream_http_client_with_options(
        Some("http://127.0.0.1:notaport"),
        false,
    )
    .err()
    .expect("M2: an unparseable proxy URL must fail to build a client");
    assert!(
        !build_err.is_empty(),
        "M2: the build failure must carry a diagnostic message"
    );

    let endpoint = spawn_plain_ok_server().await;

    // Poison every ambient proxy variable with a dead port: if the fallback
    // client inherited them, the direct dial below would fail.
    let _guard = env_lock().lock().expect("env lock");
    let mut saved: HashMap<&'static str, Option<String>> = HashMap::new();
    for key in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        saved.insert(key, std::env::var(key).ok());
    }
    for key in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        std::env::set_var(key, "http://127.0.0.1:1");
    }
    for key in ["NO_PROXY", "no_proxy"] {
        std::env::remove_var(key);
    }

    // The non-fallible factory must hand back a usable DIRECT client.
    let client = create_upstream_http_client_with_options(Some("http://127.0.0.1:notaport"), false);
    let result = client.get(&endpoint).send().await;

    // Restore the environment before asserting so a failure cannot leak it.
    for (key, value) in saved {
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
    drop(_guard);

    let resp = result.expect(
        "M2: the fallback client must dial DIRECT (explicit no_proxy) — a poisoned ambient *_PROXY made the request fail",
    );
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "M2: fallback client must reach the local upstream directly, got status {}",
        resp.status()
    );
    let body: Value = resp.json().await.expect("mock body must be JSON");
    assert_eq!(
        body,
        json!({"ok": true}),
        "M2: the direct fallback dial must hit the intended local server"
    );
}
