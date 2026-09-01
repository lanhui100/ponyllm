use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use axum::{routing::post, Router, Json};
use axum::response::IntoResponse;
use serde_json::json;
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
