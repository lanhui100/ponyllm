use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use axum::{routing::post, Router, Json};
use axum::response::IntoResponse;
use serde_json::json;
use ponyllm_core::error::CoreError;
use ponyllm_core::pool::*;
use ponyllm_core::executor::*;

/// Antigravity/Google quota exhaustion arrives as HTTP 429 with
/// RESOURCE_EXHAUSTED and the reset embedded in the message. The key must be
/// frozen for that window and the gateway must not hammer it in-request.
#[tokio::test]
async fn test_executor_quota_429_cools_key_for_advertised_reset() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();

    let app = Router::new().route("/v1/chat/completions", post(move |_headers: axum::http::HeaderMap, _body: String| {
        let cc = cc.clone();
        async move {
            cc.fetch_add(1, Ordering::SeqCst);
            (axum::http::StatusCode::TOO_MANY_REQUESTS, Json(json!({
                "error": {
                    "code": 429,
                    "message": "Individual quota reached. Please upgrade your subscription to increase your limits. Resets in 15h21m26s.",
                    "status": "RESOURCE_EXHAUSTED",
                    "details": [{
                        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                        "reason": "QUOTA_EXHAUSTED",
                        "domain": "cloudcode-pa.googleapis.com",
                        "metadata": {"uiMessage": "true", "model": "gemini-3.8-flash-high"}
                    }]
                }
            }))).into_response()
        }
    }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let endpoint = format!("http://{}/v1/chat/completions", addr);
    let pool = Arc::new(KeyPool::new("antigravity", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("ag-solo", "ag-token-1", 1, 10));

    let executor = UpstreamExecutor::new(pool.clone(), 3);
    let payload = json!({
        "model": "gemini-3.8-flash-high",
        "messages": [{"role": "user", "content": "hello"}]
    });

    let err = executor
        .execute_json_request(&endpoint, &payload)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::AllRetriesFailed {
                kind: ponyllm_core::error::GatewayErrorKind::QuotaExhausted,
                ..
            }
        ),
        "expected QuotaExhausted, got {:?}",
        err
    );

    assert_eq!(pool.get_key_status("ag-solo"), Some(KeyState::CoolingDown));
    let (remaining, reset_at) = pool.key_cooldown("ag-solo");
    let remaining = remaining.expect("cooling key must report remaining");
    assert!(
        remaining >= Duration::from_secs(15 * 3600 + 21 * 60),
        "expected ~15h21m freeze, got {:?}",
        remaining
    );
    assert!(reset_at.is_some(), "wall-clock reset must be exposed");
    // Exactly one upstream call: the closed window was never retried.
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_executor_transparent_failover_on_429() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();

    // Start a mock upstream server
    let app = Router::new().route("/v1/chat/completions", post(move |headers: axum::http::HeaderMap, _body: String| {
        let cc = cc.clone();
        async move {
            let count = cc.fetch_add(1, Ordering::SeqCst);
            let auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();

            if auth.contains("key-bad") || count == 0 {
                // First call / bad key returns 429
                (axum::http::StatusCode::TOO_MANY_REQUESTS, Json(json!({
                    "error": {"message": "Rate limit exceeded"}
                }))).into_response()
            } else {
                // Second call / good key returns 200
                (axum::http::StatusCode::OK, Json(json!({
                    "id": "chatcmpl-test",
                    "object": "chat.completion",
                    "created": 1710000000,
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "Success after failover!"},
                        "finish_reason": "stop"
                    }]
                }))).into_response()
            }
        }
    }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let endpoint = format!("http://{}/v1/chat/completions", addr);

    // Setup pool with bad key first and good key second
    let pool = Arc::new(KeyPool::new("mock-provider", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new("bad-key", "key-bad-123", 1, 10));
    pool.add_key(ApiKeyEntry::new("good-key", "key-good-456", 2, 10));

    let executor = UpstreamExecutor::new(pool.clone(), 3);
    let request_payload = json!({
        "model": "mock-model",
        "messages": [{"role": "user", "content": "hello"}]
    });

    let response = executor.execute_json_request(&endpoint, &request_payload).await.unwrap();
    assert_eq!(response["choices"][0]["message"]["content"], "Success after failover!");
    assert_eq!(call_count.load(Ordering::SeqCst), 2);

    // Bad key should now be cooling down
    assert_eq!(pool.get_key_status("bad-key").unwrap(), KeyState::CoolingDown);
}

#[tokio::test]
async fn test_executor_fails_over_across_more_keys_than_max_retries() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();

    let app = Router::new().route("/v1/chat/completions", post(move |headers: axum::http::HeaderMap, _body: String| {
        let cc = cc.clone();
        async move {
            cc.fetch_add(1, Ordering::SeqCst);
            let auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();

            if auth.contains("bad") {
                (axum::http::StatusCode::TOO_MANY_REQUESTS, Json(json!({
                    "error": {"message": "Rate limit exceeded"}
                }))).into_response()
            } else {
                (axum::http::StatusCode::OK, Json(json!({
                    "id": "chatcmpl-test",
                    "object": "chat.completion",
                    "created": 1710000000,
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "Success on 4th key!"},
                        "finish_reason": "stop"
                    }]
                }))).into_response()
            }
        }
    }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let endpoint = format!("http://{}/v1/chat/completions", addr);

    let pool = Arc::new(KeyPool::new("mock-provider", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new("bad-key-1", "key-bad-1", 1, 10));
    pool.add_key(ApiKeyEntry::new("bad-key-2", "key-bad-2", 2, 10));
    pool.add_key(ApiKeyEntry::new("bad-key-3", "key-bad-3", 3, 10));
    pool.add_key(ApiKeyEntry::new("good-key-4", "key-good-4", 4, 10));

    // max_retries is 2, but pool has 4 keys. Failover should reach good-key-4!
    let executor = UpstreamExecutor::new(pool.clone(), 2);
    let request_payload = json!({
        "model": "mock-model",
        "messages": [{"role": "user", "content": "hello"}]
    });

    let response = executor.execute_json_request(&endpoint, &request_payload).await.unwrap();
    assert_eq!(response["choices"][0]["message"]["content"], "Success on 4th key!");
    assert_eq!(call_count.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn test_all_keys_failed_reports_attempted_keys_trajectory() {
    let app = Router::new().route("/v1/chat/completions", post(move |_headers: axum::http::HeaderMap, _body: String| {
        async move {
            (axum::http::StatusCode::TOO_MANY_REQUESTS, Json(json!({
                "error": {"message": "Rate limit exceeded"}
            }))).into_response()
        }
    }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let endpoint = format!("http://{}/v1/chat/completions", addr);

    let pool = Arc::new(KeyPool::new("mock-provider", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new("key-alpha", "key-1", 1, 10));
    pool.add_key(ApiKeyEntry::new("key-beta", "key-2", 2, 10));
    pool.add_key(ApiKeyEntry::new("key-gamma", "key-3", 3, 10));

    let executor = UpstreamExecutor::new(pool.clone(), 3);
    let request_payload = json!({
        "model": "mock-model",
        "messages": [{"role": "user", "content": "hello"}]
    });

    let err = executor.execute_json_request(&endpoint, &request_payload).await.unwrap_err();
    let err_msg = err.to_string();
    assert!(err_msg.contains("key-alpha"), "Error message should mention key-alpha, got: {}", err_msg);
    assert!(err_msg.contains("key-beta"), "Error message should mention key-beta, got: {}", err_msg);
    assert!(err_msg.contains("key-gamma"), "Error message should mention key-gamma, got: {}", err_msg);
    assert!(err_msg.contains("failures: 3 rate limited"), "Error message should summarize failures, got: {}", err_msg);
}

#[test]
fn test_select_key_excluding_priority_and_exhaustion() {
    let pool = KeyPool::new("test-provider", RoutingStrategy::Priority);
    pool.add_key(ApiKeyEntry::new("k1", "key-1", 1, 10));
    pool.add_key(ApiKeyEntry::new("k2", "key-2", 2, 10));
    pool.add_key(ApiKeyEntry::new("k3", "key-3", 3, 10));

    // Initially k1
    let key = pool.select_key_excluding(&[]).unwrap();
    assert_eq!(key.id, "k1");

    // Excluding k1 yields k2
    let key = pool.select_key_excluding(&["k1".to_string()]).unwrap();
    assert_eq!(key.id, "k2");

    // Excluding k1 and k2 yields k3
    let key = pool.select_key_excluding(&["k1".to_string(), "k2".to_string()]).unwrap();
    assert_eq!(key.id, "k3");

    // Excluding all yields NoAvailableKey
    let err = pool.select_key_excluding(&["k1".to_string(), "k2".to_string(), "k3".to_string()]).unwrap_err();
    assert!(matches!(err, CoreError::NoAvailableKey(_)));
}

#[tokio::test]
async fn test_executor_fails_over_on_server_error_without_immediate_cooldown() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();

    // Mock upstream returns 500 on bad keys, 200 on good key
    // Server error does NOT immediately cooldown the key (requires 3 consecutive failures)
    let app = Router::new().route("/v1/chat/completions", post(move |headers: axum::http::HeaderMap, _body: String| {
        let cc = cc.clone();
        async move {
            cc.fetch_add(1, Ordering::SeqCst);
            let auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();

            if auth.contains("bad") {
                (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "upstream temporary 500").into_response()
            } else {
                (axum::http::StatusCode::OK, Json(json!({
                    "id": "chatcmpl-test",
                    "object": "chat.completion",
                    "created": 1710000000,
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "Success after non-cooldown error failover!"},
                        "finish_reason": "stop"
                    }]
                }))).into_response()
            }
        }
    }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let endpoint = format!("http://{}/v1/chat/completions", addr);

    // Setup pool with Priority strategy where bad keys would stay Active
    let pool = Arc::new(KeyPool::new("mock-provider", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new("bad-key-1", "key-bad-1", 1, 10));
    pool.add_key(ApiKeyEntry::new("bad-key-2", "key-bad-2", 2, 10));
    pool.add_key(ApiKeyEntry::new("good-key-3", "key-good-3", 3, 10));

    let executor = UpstreamExecutor::new(pool.clone(), 3);
    let request_payload = json!({
        "model": "mock-model",
        "messages": [{"role": "user", "content": "hello"}]
    });

    // Without select_key_excluding, bad-key-1 would be retried 3 times because it is still Active!
    // With select_key_excluding, bad-key-1 and bad-key-2 are excluded, reaching good-key-3!
    let response = executor.execute_json_request(&endpoint, &request_payload).await.unwrap();
    assert_eq!(response["choices"][0]["message"]["content"], "Success after non-cooldown error failover!");
    assert_eq!(call_count.load(Ordering::SeqCst), 3);
}

#[test]
fn test_transient_geo_gate_signature() {
    // Genuine caller errors never match.
    assert!(!is_transient_geo_gate(400, r#"{"error":{"message":"messages must not be empty"}}"#));
    assert!(!is_transient_geo_gate(429, "User location is not supported for the API use."));
    // Google geo-gate shape matches (case-insensitive).
    assert!(is_transient_geo_gate(400, r#"{"error":{"code":400,"message":"User location is not supported for the API use.","status":"FAILED_PRECONDITION"}}"#));
    assert!(is_transient_geo_gate(400, "400 FAILED_PRECONDITION: UNSUPPORTED_LOCATION"));
}

#[tokio::test]
async fn test_singleton_retries_transient_geo_gate_once() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();

    // First call hits the transient geo-gate, second succeeds.
    let app = Router::new().route("/v1/chat/completions", post(move |_headers: axum::http::HeaderMap, _body: String| {
        let cc = cc.clone();
        async move {
            let n = cc.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                (axum::http::StatusCode::BAD_REQUEST, Json(json!({
                    "error": {"code": 400, "message": "User location is not supported for the API use.", "status": "FAILED_PRECONDITION"}
                }))).into_response()
            } else {
                (axum::http::StatusCode::OK, Json(json!({
                    "id": "chatcmpl-test",
                    "object": "chat.completion",
                    "created": 1710000000,
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "Recovered after geo blip!"},
                        "finish_reason": "stop"
                    }]
                }))).into_response()
            }
        }
    }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let endpoint = format!("http://{}/v1/chat/completions", addr);

    let pool = Arc::new(KeyPool::new("mock-provider", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new("only-key", "key-1", 1, 10));

    let executor = UpstreamExecutor::new(pool.clone(), 3);
    let request_payload = json!({
        "model": "mock-model",
        "messages": [{"role": "user", "content": "hello"}]
    });

    let response = executor.execute_json_request(&endpoint, &request_payload).await.unwrap();
    assert_eq!(response["choices"][0]["message"]["content"], "Recovered after geo blip!");
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
    // Geo-gate records no pool state: the key stays healthy.
    assert_eq!(pool.get_key_status("only-key").unwrap(), KeyState::Active);
}

#[tokio::test]
async fn test_genuine_400_stays_terminal_without_retry() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();

    let app = Router::new().route("/v1/chat/completions", post(move |_headers: axum::http::HeaderMap, _body: String| {
        let cc = cc.clone();
        async move {
            cc.fetch_add(1, Ordering::SeqCst);
            (axum::http::StatusCode::BAD_REQUEST, Json(json!({
                "error": {"message": "messages must not be empty", "type": "invalid_request_error"}
            }))).into_response()
        }
    }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let endpoint = format!("http://{}/v1/chat/completions", addr);

    let pool = Arc::new(KeyPool::new("mock-provider", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new("only-key", "key-1", 1, 10));

    let executor = UpstreamExecutor::new(pool.clone(), 3);
    let request_payload = json!({
        "model": "mock-model",
        "messages": [{"role": "user", "content": "hello"}]
    });

    let err = executor.execute_json_request(&endpoint, &request_payload).await.unwrap_err();
    assert!(matches!(err, CoreError::UpstreamStatusError { .. }));
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_create_upstream_http_client_options() {
    use ponyllm_core::executor::{create_upstream_http_client, create_upstream_http_client_with_options};

    // Default client builds successfully with no_proxy
    let default_client = create_upstream_http_client();
    let _ = default_client;

    // Explicit proxy configuration builds successfully
    let proxy_client = create_upstream_http_client_with_options(Some("http://127.0.0.1:8899"), false);
    let _ = proxy_client;

    // System proxy enabled builds successfully
    let sys_proxy_client = create_upstream_http_client_with_options(None, true);
    let _ = sys_proxy_client;
}

#[test]
fn test_detect_system_proxy() {
    use ponyllm_core::executor::detect_system_proxy;

    let keys = [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];
    let saved: Vec<(&str, Option<String>)> = keys.iter().map(|&k| (k, std::env::var(k).ok())).collect();

    // Clear ambient env vars
    for &k in &keys {
        std::env::remove_var(k);
    }

    // Test with environment variable override
    std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:9099");
    let detected = detect_system_proxy();
    assert_eq!(detected.as_deref(), Some("http://127.0.0.1:9099"));
    std::env::remove_var("HTTPS_PROXY");

    // Test port listening probe
    let listener = std::net::TcpListener::bind("127.0.0.1:8899");
    if let Ok(_l) = listener {
        // When 8899 is bound and no env vars set
        let detected = detect_system_proxy();
        assert_eq!(detected.as_deref(), Some("http://127.0.0.1:8899"));
    }

    // Restore environment
    for (k, val) in saved {
        if let Some(v) = val {
            std::env::set_var(k, v);
        } else {
            std::env::remove_var(k);
        }
    }
}

#[tokio::test]
async fn test_executor_ttfb_timeout_override_and_disabled() {
    assert_eq!(
        DEFAULT_UPSTREAM_TTFB_TIMEOUT,
        Duration::from_secs(90),
        "Default TTFB timeout must be 90s"
    );

    let app = Router::new().route(
        "/v1/chat/completions",
        post(|_body: String| async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            (
                axum::http::StatusCode::OK,
                Json(json!({
                    "id": "chatcmpl-test",
                    "object": "chat.completion",
                    "choices": [{"message": {"role": "assistant", "content": "pong"}}]
                })),
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
    let pool = Arc::new(KeyPool::new("test-prov", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let payload = json!({"messages": [{"role": "user", "content": "ping"}]});

    // 1. Tight TTFB timeout (50ms) must fail when server sleeps 200ms
    let tight_exec = UpstreamExecutor::new(pool.clone(), 1)
        .with_ttfb_timeout(Some(Duration::from_millis(50)));
    let err = tight_exec
        .execute_json_request(&endpoint, &payload)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("upstream TTFB timeout after 50ms"),
        "expected TTFB timeout error message, got {}",
        err
    );

    // 2. Generous TTFB timeout (500ms) must succeed
    let generous_exec = UpstreamExecutor::new(pool.clone(), 1)
        .with_ttfb_timeout(Some(Duration::from_millis(500)));
    let resp = generous_exec
        .execute_json_request(&endpoint, &payload)
        .await
        .unwrap();
    assert_eq!(resp["choices"][0]["message"]["content"], "pong");

    // 3. Disabled TTFB timeout (None) must succeed
    let disabled_exec = UpstreamExecutor::new(pool.clone(), 1)
        .with_ttfb_timeout(None);
    let resp_disabled = disabled_exec
        .execute_json_request(&endpoint, &payload)
        .await
        .unwrap();
    assert_eq!(resp_disabled["choices"][0]["message"]["content"], "pong");
}

#[tokio::test]
async fn test_executor_ttfb_timeout_multi_key_failover() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|headers: axum::http::HeaderMap| async move {
            let auth = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if auth.contains("sk-k1") {
                // Key 1 triggers TTFB timeout
                tokio::time::sleep(Duration::from_millis(200)).await;
                (
                    axum::http::StatusCode::OK,
                    Json(json!({"choices": [{"message": {"content": "from-k1"}}]})),
                )
                    .into_response()
            } else {
                // Key 2 succeeds within budget
                (
                    axum::http::StatusCode::OK,
                    Json(json!({"choices": [{"message": {"content": "from-k2"}}]})),
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

    let endpoint = format!("http://{}/v1/chat/completions", addr);
    let pool = Arc::new(KeyPool::new("test-prov", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new("k1", "sk-k1", 1, 10));
    pool.add_key(ApiKeyEntry::new("k2", "sk-k2", 10, 10));
    let payload = json!({"messages": [{"role": "user", "content": "ping"}]});

    // UpstreamExecutor with 2 attempts and 50ms TTFB timeout
    let exec = UpstreamExecutor::new(pool.clone(), 2)
        .with_ttfb_timeout(Some(Duration::from_millis(50)));
    let resp = exec
        .execute_json_request(&endpoint, &payload)
        .await
        .unwrap();

    // Must successfully fail over to k2
    assert_eq!(resp["choices"][0]["message"]["content"], "from-k2");
    // Verify k1 error and k2 success recorded
    let keys = pool.snapshot_keys();
    let k1 = keys.iter().find(|k| k.id == "k1").unwrap();
    assert_eq!(k1.stats.failed_requests.load(Ordering::SeqCst), 1);
    let k2 = keys.iter().find(|k| k.id == "k2").unwrap();
    assert_eq!(k2.stats.successful_requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_executor_ttfb_timeout_stream_request() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            (
                [("content-type", "text/event-stream")],
                "data: {\"choices\": [{\"delta\": {\"content\": \"hello\"}}]}\n\ndata: [DONE]\n\n",
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
    let pool = Arc::new(KeyPool::new("test-prov", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let payload = json!({"stream": true, "messages": [{"role": "user", "content": "ping"}]});

    // 1. Tight TTFB (50ms) on stream request must timeout
    let tight_exec = UpstreamExecutor::new(pool.clone(), 1)
        .with_ttfb_timeout(Some(Duration::from_millis(50)));
    let err = tight_exec
        .execute_stream_request(&endpoint, &payload)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("upstream TTFB timeout after 50ms"),
        "expected TTFB timeout for stream request, got {}",
        err
    );

    // 2. Generous TTFB (500ms) on stream request must succeed
    let generous_exec = UpstreamExecutor::new(pool.clone(), 1)
        .with_ttfb_timeout(Some(Duration::from_millis(500)));
    let resp = generous_exec
        .execute_stream_request(&endpoint, &payload)
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_executor_ttfb_timeout_does_not_kill_slow_body_streaming() {
    use bytes::Bytes;

    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async move {
            let stream = futures::stream::unfold(0, |count| async move {
                if count >= 3 {
                    None
                } else {
                    if count > 0 {
                        // Delay between body chunks
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    let chunk = format!("data: chunk {}\n\n", count);
                    Some((Ok::<Bytes, std::convert::Infallible>(Bytes::from(chunk)), count + 1))
                }
            });
            (
                [("content-type", "text/event-stream")],
                axum::body::Body::from_stream(stream),
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
    let pool = Arc::new(KeyPool::new("test-prov", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let payload = json!({"stream": true, "messages": [{"role": "user", "content": "ping"}]});

    // TTFB timeout is 60ms. Total body streaming takes ~100ms.
    // Since headers arrive immediately (TTFB < 10ms), this must NOT timeout!
    let exec = UpstreamExecutor::new(pool.clone(), 1)
        .with_ttfb_timeout(Some(Duration::from_millis(60)));
    let resp = exec
        .execute_stream_request(&endpoint, &payload)
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let full_body = resp.text().await.unwrap();
    assert!(full_body.contains("chunk 0"));
    assert!(full_body.contains("chunk 1"));
    assert!(full_body.contains("chunk 2"));
}

#[test]
fn test_summarize_attempt_failures_distinguishes_lock_contention() {
    use ponyllm_core::error::GatewayErrorKind;
    use ponyllm_core::executor::summarize_attempt_failures;

    // 1. Only lock contention: must NOT report timeout/network
    let summary1 = summarize_attempt_failures(&[
        GatewayErrorKind::LockContention,
        GatewayErrorKind::LockContention,
    ]);
    assert_eq!(summary1, " (failures: 2 lock busy/contention)");
    assert!(!summary1.contains("timeout/network"));

    // 2. Mixed lock contention and timeout/network: both reported independently
    let summary2 = summarize_attempt_failures(&[
        GatewayErrorKind::UpstreamUnavailable,
        GatewayErrorKind::LockContention,
        GatewayErrorKind::RateLimitExceeded { retry_after: None },
    ]);
    assert_eq!(
        summary2,
        " (failures: 1 timeout/network, 1 lock busy/contention, 1 rate limited)"
    );

    // 3. Only timeout/network: does not mention lock busy/contention
    let summary3 = summarize_attempt_failures(&[GatewayErrorKind::UpstreamUnavailable]);
    assert_eq!(summary3, " (failures: 1 timeout/network)");
    assert!(!summary3.contains("lock busy"));
}

#[tokio::test]
async fn test_refresh_skipped_produces_lock_contention_in_executor() {
    use ponyllm_core::pool::refresh_gate::{RefreshGate, RefreshGateGuard, RefreshGateError};
    use ponyllm_core::error::GatewayErrorKind;

    #[derive(Debug, Default)]
    struct MockSkipGate;
    impl RefreshGateGuard for MockSkipGate {}

    #[async_trait::async_trait]
    impl RefreshGate for MockSkipGate {
        async fn try_acquire(
            &self,
            _key_id: &str,
        ) -> std::result::Result<Option<Box<dyn RefreshGateGuard + Send + Sync>>, RefreshGateError> {
            Ok(None) // Always simulate another replica holding the lock
        }
    }

    let cred = AntigravityCredential {
        access_token: None, // Missing token forces refresh
        refresh_token: "rf-token".to_string(),
        client_id: "client-id".to_string(),
        client_secret: "client-secret".to_string(),
        project_id: "proj-1".to_string(),
        expiry: None,
    };
    let mgr = Arc::new(AntigravityTokenManager::new(
        "ag-lock-busy-key",
        cred,
        reqwest::Client::new(),
    ));
    mgr.set_refresh_gate(Some(Arc::new(MockSkipGate)));

    let pool = Arc::new(KeyPool::new("antigravity-prov", RoutingStrategy::RoundRobin));
    let key = ApiKeyEntry::new_antigravity("ag-lock-busy-key", mgr, 1, 10);
    pool.add_key(key);

    let exec = UpstreamExecutor::new(pool.clone(), 1);
    let payload = json!({"messages": [{"role": "user", "content": "ping"}]});
    let err = exec
        .execute_json_request("https://api.example.com/v1/chat/completions", &payload)
        .await
        .unwrap_err();

    // 1. Error kind must be LockContention, NOT UpstreamUnavailable
    assert_eq!(err.kind(), GatewayErrorKind::LockContention);

    // 2. Formatted message must contain "lock busy/contention" and NOT "timeout/network"
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("lock busy/contention"),
        "expected 'lock busy/contention' in error message, got: {}",
        err_msg
    );
    assert!(
        !err_msg.contains("timeout/network"),
        "error message should NOT report lock contention as timeout/network, got: {}",
        err_msg
    );

    // 3. Healthy key encountering lock contention MUST remain Active (never cooled)
    let key_entry = pool.snapshot_keys().into_iter().find(|k| k.id == "ag-lock-busy-key").unwrap();
    assert_eq!(key_entry.current_state(), ponyllm_core::pool::KeyState::Active);
}

#[tokio::test]
async fn test_singleflight_propagates_refresh_skipped_without_leaking_internal() {
    use ponyllm_core::pool::refresh_gate::{RefreshGate, RefreshGateGuard, RefreshGateError};

    #[derive(Debug, Default)]
    struct DelayedSkipGate;
    impl RefreshGateGuard for DelayedSkipGate {}

    #[async_trait::async_trait]
    impl RefreshGate for DelayedSkipGate {
        async fn try_acquire(
            &self,
            _key_id: &str,
        ) -> std::result::Result<Option<Box<dyn RefreshGateGuard + Send + Sync>>, RefreshGateError> {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok(None)
        }
    }

    let cred = AntigravityCredential {
        access_token: None,
        refresh_token: "rf-token".to_string(),
        client_id: "client-id".to_string(),
        client_secret: "client-secret".to_string(),
        project_id: "proj-1".to_string(),
        expiry: None,
    };
    let mgr = Arc::new(AntigravityTokenManager::new(
        "ag-concurrent-key",
        cred,
        reqwest::Client::new(),
    ));
    mgr.set_refresh_gate(Some(Arc::new(DelayedSkipGate)));

    let m1 = mgr.clone();
    let m2 = mgr.clone();

    let (res1, res2) = tokio::join!(
        tokio::spawn(async move { m1.get_valid_token().await }),
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            m2.get_valid_token().await
        }),
    );

    let err1 = res1.unwrap().unwrap_err();
    let err2 = res2.unwrap().unwrap_err();

    // BOTH leader and follower MUST receive RefreshSkipped, NOT Internal!
    assert!(matches!(err1, CoreError::RefreshSkipped { .. }), "leader got: {:?}", err1);
    assert!(matches!(err2, CoreError::RefreshSkipped { .. }), "follower got: {:?}", err2);
}

#[tokio::test]
async fn test_antigravity_401_recovery_lock_contention_does_not_burn_key() {
    use ponyllm_core::error::GatewayErrorKind;
    use ponyllm_core::pool::refresh_gate::{RefreshGate, RefreshGateGuard, RefreshGateError};
    use ponyllm_core::pool::KeyState;

    // Upstream server returns 401
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            if let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let resp = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 26\r\n\r\n{\"error\": \"invalid_token\"}";
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        }
    });

    #[derive(Debug, Default)]
    struct MockSkipGate;
    impl RefreshGateGuard for MockSkipGate {}

    #[async_trait::async_trait]
    impl RefreshGate for MockSkipGate {
        async fn try_acquire(
            &self,
            _key_id: &str,
        ) -> std::result::Result<Option<Box<dyn RefreshGateGuard + Send + Sync>>, RefreshGateError> {
            Ok(None)
        }
    }

    let cred = AntigravityCredential {
        access_token: Some("stale-token".to_string()),
        refresh_token: "rf-token".to_string(),
        client_id: "client-id".to_string(),
        client_secret: "client-secret".to_string(),
        project_id: "proj-1".to_string(),
        expiry: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
    };
    let mgr = Arc::new(AntigravityTokenManager::new(
        "ag-401-key",
        cred,
        reqwest::Client::new(),
    ));
    mgr.set_refresh_gate(Some(Arc::new(MockSkipGate)));

    let pool = Arc::new(KeyPool::new("antigravity-prov", RoutingStrategy::RoundRobin));
    let key = ApiKeyEntry::new_antigravity("ag-401-key", mgr.clone(), 1, 10);
    pool.add_key(key);

    let exec = UpstreamExecutor::new(pool.clone(), 1);
    let payload = json!({"messages": [{"role": "user", "content": "ping"}]});
    let target_url = format!("http://{}/v1/chat/completions", addr);
    let err = exec
        .execute_json_request(&target_url, &payload)
        .await
        .unwrap_err();

    assert_eq!(err.kind(), GatewayErrorKind::LockContention);

    // Stale token in memory should be invalidated
    assert!(mgr.credential_snapshot().access_token.is_none());

    // The key MUST NOT be cooling or burned (Active)!
    let key_entry = pool.snapshot_keys().into_iter().find(|k| k.id == "ag-401-key").unwrap();
    assert_eq!(key_entry.current_state(), KeyState::Active);
}

#[tokio::test]
async fn test_executor_mixed_lock_contention_and_network_timeout() {
    use ponyllm_core::pool::refresh_gate::{RefreshGate, RefreshGateGuard, RefreshGateError};
    use ponyllm_core::pool::KeyState;

    #[derive(Debug, Default)]
    struct MockSkipGate;
    impl RefreshGateGuard for MockSkipGate {}

    #[async_trait::async_trait]
    impl RefreshGate for MockSkipGate {
        async fn try_acquire(
            &self,
            _key_id: &str,
        ) -> std::result::Result<Option<Box<dyn RefreshGateGuard + Send + Sync>>, RefreshGateError> {
            Ok(None)
        }
    }

    let cred = AntigravityCredential {
        access_token: None,
        refresh_token: "rf-token".to_string(),
        client_id: "client-id".to_string(),
        client_secret: "client-secret".to_string(),
        project_id: "proj-1".to_string(),
        expiry: None,
    };
    let mgr = Arc::new(AntigravityTokenManager::new(
        "key-1-lock",
        cred,
        reqwest::Client::new(),
    ));
    mgr.set_refresh_gate(Some(Arc::new(MockSkipGate)));

    let pool = Arc::new(KeyPool::new("prov", RoutingStrategy::RoundRobin));
    // Key 1: Antigravity key with lock contention (remains Active)
    let key1 = ApiKeyEntry::new_antigravity("key-1-lock", mgr, 1, 10);
    // Key 2: Regular key pointing to unreachable network target (will cool down after reaching threshold)
    let key2 = ApiKeyEntry::new("key-2-net", "sk-test", 1, 10);
    key2.stats.consecutive_failures.store(2, std::sync::atomic::Ordering::Relaxed);
    pool.add_key(key1);
    pool.add_key(key2);

    let exec = UpstreamExecutor::new(pool.clone(), 2);
    let payload = json!({"messages": [{"role": "user", "content": "ping"}]});
    // Port 1 is closed/unreachable
    let err = exec
        .execute_json_request("http://127.0.0.1:1/v1/chat/completions", &payload)
        .await
        .unwrap_err();

    let err_msg = err.to_string();
    assert!(err_msg.contains("1 lock busy/contention"), "got: {}", err_msg);
    assert!(err_msg.contains("1 timeout/network"), "got: {}", err_msg);

    // Key 1 must be Active (not cooled)
    let k1 = pool.snapshot_keys().into_iter().find(|k| k.id == "key-1-lock").unwrap();
    assert_eq!(k1.current_state(), KeyState::Active);

    // Key 2 must be CoolingDown (network error cooled it)
    let k2 = pool.snapshot_keys().into_iter().find(|k| k.id == "key-2-net").unwrap();
    assert_eq!(k2.current_state(), KeyState::CoolingDown);
}

#[tokio::test]
async fn test_streaming_refresh_skipped_produces_lock_contention() {
    use ponyllm_core::error::GatewayErrorKind;
    use ponyllm_core::pool::refresh_gate::{RefreshGate, RefreshGateGuard, RefreshGateError};
    use ponyllm_core::pool::KeyState;

    #[derive(Debug, Default)]
    struct MockSkipGate;
    impl RefreshGateGuard for MockSkipGate {}

    #[async_trait::async_trait]
    impl RefreshGate for MockSkipGate {
        async fn try_acquire(
            &self,
            _key_id: &str,
        ) -> std::result::Result<Option<Box<dyn RefreshGateGuard + Send + Sync>>, RefreshGateError> {
            Ok(None)
        }
    }

    let cred = AntigravityCredential {
        access_token: None,
        refresh_token: "rf-token".to_string(),
        client_id: "client-id".to_string(),
        client_secret: "client-secret".to_string(),
        project_id: "proj-1".to_string(),
        expiry: None,
    };
    let mgr = Arc::new(AntigravityTokenManager::new(
        "ag-stream-lock-key",
        cred,
        reqwest::Client::new(),
    ));
    mgr.set_refresh_gate(Some(Arc::new(MockSkipGate)));

    let pool = Arc::new(KeyPool::new("antigravity-prov", RoutingStrategy::RoundRobin));
    let key = ApiKeyEntry::new_antigravity("ag-stream-lock-key", mgr, 1, 10);
    pool.add_key(key);

    let exec = UpstreamExecutor::new(pool.clone(), 1);
    let payload = json!({"messages": [{"role": "user", "content": "ping"}], "stream": true});
    let err = exec
        .execute_stream_request_with_timing_and_key("https://api.example.com/v1/chat/completions", &payload)
        .await
        .unwrap_err();

    assert_eq!(err.kind(), GatewayErrorKind::LockContention);
    let key_entry = pool.snapshot_keys().into_iter().find(|k| k.id == "ag-stream-lock-key").unwrap();
    assert_eq!(key_entry.current_state(), KeyState::Active);
}

#[tokio::test]
async fn test_singleflight_post_gate_recheck_notifies_followers() {
    use ponyllm_core::pool::refresh_gate::{RefreshGate, RefreshGateGuard, RefreshGateError};
    use ponyllm_core::pool::KeyState;

    #[derive(Debug, Default)]
    struct DummyGuard;
    impl RefreshGateGuard for DummyGuard {}

    struct RecheckSimulatingGate {
        mgr: Arc<parking_lot::Mutex<Option<Arc<AntigravityTokenManager>>>>,
    }

    impl std::fmt::Debug for RecheckSimulatingGate {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "RecheckSimulatingGate")
        }
    }

    #[async_trait::async_trait]
    impl RefreshGate for RecheckSimulatingGate {
        async fn try_acquire(
            &self,
            _key_id: &str,
        ) -> std::result::Result<Option<Box<dyn RefreshGateGuard + Send + Sync>>, RefreshGateError> {
            // Wait so follower coroutine can subscribe to singleflight broadcast
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            if let Some(m) = self.mgr.lock().as_ref() {
                m.update_credential_for_test(|c| {
                    c.access_token = Some("post-gate-valid-token-777".to_string());
                    c.expiry = Some(chrono::Utc::now() + chrono::Duration::hours(1));
                });
            }
            Ok(Some(Box::new(DummyGuard)))
        }
    }

    let cred = AntigravityCredential {
        access_token: None, // Missing token forces refresh
        refresh_token: "rf-token".to_string(),
        client_id: "client-id".to_string(),
        client_secret: "client-secret".to_string(),
        project_id: "proj-1".to_string(),
        expiry: None,
    };
    let mgr_holder = Arc::new(parking_lot::Mutex::new(None));
    let mgr = Arc::new(AntigravityTokenManager::new(
        "ag-recheck-key",
        cred,
        reqwest::Client::new(),
    ));
    *mgr_holder.lock() = Some(mgr.clone());
    mgr.set_refresh_gate(Some(Arc::new(RecheckSimulatingGate { mgr: mgr_holder })));

    let pool = Arc::new(KeyPool::new("prov", RoutingStrategy::RoundRobin));
    let key = ApiKeyEntry::new_antigravity("ag-recheck-key", mgr.clone(), 1, 10);
    pool.add_key(key);

    let m1 = mgr.clone();
    let m2 = mgr.clone();

    let (res1, res2) = tokio::join!(
        tokio::spawn(async move { m1.get_valid_token().await }),
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            m2.get_valid_token().await
        }),
    );

    let tok1 = res1.unwrap().expect("leader got valid token via post-gate recheck");
    let tok2 = res2.unwrap().expect("follower got valid token via broadcast from leader");

    assert_eq!(tok1, "post-gate-valid-token-777");
    assert_eq!(tok2, "post-gate-valid-token-777");

    let key_entry = pool.snapshot_keys().into_iter().find(|k| k.id == "ag-recheck-key").unwrap();
    assert_eq!(key_entry.current_state(), KeyState::Active);
    assert_eq!(key_entry.stats.consecutive_failures.load(std::sync::atomic::Ordering::Relaxed), 0);
}




