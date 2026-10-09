use axum::{routing::post, Json, Router};
use ponyllm_core::sentry::{SentryClient, SentryConfig};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

#[tokio::test]
async fn test_sentry_client_green_phase() {
    let received_count = Arc::new(AtomicUsize::new(0));
    let received_count_clone = received_count.clone();

    let app = Router::new().route(
        "/api/v1/ingest",
        post(move |Json(payload): Json<Value>| {
            let count = received_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                let str_val = payload.to_string();
                assert!(
                    !str_val.contains("sk-secret-test-key-12345"),
                    "Sensitive key leaked!"
                );
                assert!(
                    str_val.contains("[REDACTED_API_KEY]"),
                    "Redaction marker missing"
                );
                (
                    axum::http::StatusCode::OK,
                    Json(serde_json::json!({
                        "issue_id": "issue-1",
                        "event_id": "event-1",
                        "fingerprint": "fp-1",
                        "status": "unresolved",
                        "count": 1
                    })),
                )
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = SentryConfig {
        endpoint: format!("http://127.0.0.1:{}/api/v1/ingest", port),
        client_token: Some("test-token".into()),
        environment: Some("test".into()),
        release: Some("0.1.0".into()),
        buffer_capacity: 100,
    };

    let client = SentryClient::new(config);
    let mut tags = HashMap::new();
    tags.insert("provider".to_string(), "deepseek".to_string());
    client.capture_error(
        "RateLimitExceeded",
        "Rate limit on sk-secret-test-key-12345",
        Some(tags),
        None,
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(received_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_sentry_client_resilience_when_down() {
    let config = SentryConfig {
        endpoint: "http://127.0.0.1:59999/api/v1/ingest".into(),
        client_token: None,
        environment: Some("test".into()),
        release: Some("0.1.0".into()),
        buffer_capacity: 10,
    };

    let client = SentryClient::new(config);
    for i in 0..20 {
        client.capture_error(
            "UpstreamUnavailable",
            &format!("Upstream error {}", i),
            None,
            None,
        );
    }
}
