//! FROZEN RED-PHASE ACCEPTANCE — Wave-2b (contract v6, review-driven), core
//! slice:
//!
//! 4. **requestId header/body consistency**: N1 rewrites the Antigravity
//!    `requestId` HEADER when the client value is illegal, but the request BODY
//!    still carries the client value. Upstream sees two different ids (header
//!    vs body) — v6 requires the body to be rewritten to the same gateway id.
//! 5. **fallback client timeouts**: the direct-dial fallback client built when
//!    a proxy cannot be constructed must BOUD its waits (connect ≤ ~5s) — an
//!    un-timed fallback can hang a request forever on a black-holed upstream.
//!
//! Both tests are behavioural (real HTTP / real client), no `#[ignore]`, no
//! sleeps used as assertions.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::executor::{create_upstream_http_client_with_options, UpstreamExecutor};
use ponyllm_core::pool::{
    AntigravityCredential, AntigravityTokenManager, ApiKeyEntry, KeyPool, RoutingStrategy,
};
use serde_json::{json, Value};

const REQUEST_ID_ALLOWED: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_.:/-";

/// What the mock upstream saw for one attempt.
#[derive(Debug, Clone)]
struct Observed {
    header_request_id: Option<String>,
    body: Value,
}

async fn spawn_upstream_capture() -> (String, Arc<Mutex<Vec<Observed>>>) {
    let seen: Arc<Mutex<Vec<Observed>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: axum::http::HeaderMap, body: String| {
            let sink = sink.clone();
            async move {
                let header_request_id = headers
                    .get("requestid")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                sink.lock().expect("capture lock").push(Observed {
                    header_request_id,
                    body: serde_json::from_str(&body).unwrap_or(Value::Null),
                });
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
    (format!("http://{addr}/v1/chat/completions"), seen)
}

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

async fn observe_request(client_request_id: &str) -> Observed {
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
        "v6: exactly one upstream attempt expected, got {}",
        captured.len()
    );
    captured[0].clone()
}

// ---------------------------------------------------------------------------
// 4. requestId header/body consistency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn v6_illegal_request_id_yields_one_identical_value_in_header_and_body() {
    for hostile in [
        "agent/u/1/traj-1/1\r\nX-Injected: evil", // CRLF
        "agent/u/1/traj 1/1",                      // space
        "agent/u/1/traj-1/1;rm -rf",               // illegal char
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 200 chars
    ] {
        let observed = observe_request(hostile).await;
        let header = observed
            .header_request_id
            .clone()
            .unwrap_or_else(|| panic!("v6: gateway must always send a requestId header (client value: {hostile:?})"));
        let body_id = observed
            .body
            .get("requestId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "v6: upstream body must carry a requestId (client value {hostile:?}, body = {})",
                    observed.body
                )
            })
            .to_string();

        assert_ne!(
            header, hostile,
            "v6: an illegal client requestId must be replaced; header still holds the client value"
        );
        assert_eq!(
            body_id, header,
            "v6: header and body must carry the SAME requestId (header = {header:?}, body = {body_id:?})"
        );
        assert!(
            header.starts_with("agent/"),
            "v6: the shared value must be the gateway-generated `agent/…` form, got '{header}'"
        );
        assert!(
            header.chars().all(|c| REQUEST_ID_ALLOWED.contains(c)),
            "v6: the shared value must stay inside the accepted charset, got '{header}'"
        );
    }
}

#[tokio::test]
async fn v6_legal_request_id_is_identical_in_header_and_body() {
    let legal = "agent/01HXYZABCD/1700000000000/traj-ab/1";
    let observed = observe_request(legal).await;
    assert_eq!(
        observed.header_request_id.as_deref(),
        Some(legal),
        "v6: a legal client requestId must be forwarded verbatim in the header"
    );
    assert_eq!(
        observed.body.get("requestId").and_then(Value::as_str),
        Some(legal),
        "v6: the body must carry the same legal requestId as the header"
    );
}

// ---------------------------------------------------------------------------
// 5. fallback client bounds its connect wait
// ---------------------------------------------------------------------------

#[tokio::test]
async fn v6_fallback_client_bounds_connect_wait() {
    // Trigger the fallback with an unbuildable proxy (see
    // crates/ponyllm-core/tests/security_p1_acceptance.rs for the socks-scheme
    // policy, which lives in validate_egress_entry).
    let client = create_upstream_http_client_with_options(Some("http://127.0.0.1:notaport"), false);

    // RFC 5737 TEST-NET-1 is black-holed by design: a client without a connect
    // timeout waits forever, while one bounded at ≤5s errors well inside this
    // envelope. 6s gives the 5s semantic one second of scheduling margin.
    let bound = Duration::from_secs(6);
    let started = Instant::now();
    let outcome = tokio::time::timeout(bound, client.get("http://192.0.2.1:81/").send()).await;
    let elapsed = started.elapsed();

    let result = outcome.unwrap_or_else(|_| {
        panic!(
            "v6: the fallback client must bound its connect wait (≤5s); it was still connecting after {bound:?}"
        )
    });
    let err = result.expect_err("v6: a black-holed TEST-NET-1 target must not answer");
    assert!(
        elapsed <= bound,
        "v6: the connect failure must surface within {bound:?}, took {elapsed:?}"
    );
    // The error must be a transport/connect/timeout family error, never a panic
    // and never a silent success.
    assert!(
        err.is_connect() || err.is_timeout() || err.is_request(),
        "v6: expected a transport-level error from a black-holed target, got {err:?}"
    );
}
