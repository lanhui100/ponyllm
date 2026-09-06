use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use axum::{routing::post, Router, Json};
use axum::response::IntoResponse;
use serde_json::json;
use ponyllm_core::error::CoreError;
use ponyllm_core::pool::*;
use ponyllm_core::executor::*;

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

