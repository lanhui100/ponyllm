use ponyllm_core::sentry::{Exception, Frame, RawEvent, SentryClient, SentryConfig};
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_sentry_serialization_and_transmission() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/1/store/"))
        .and(header("x-client-token", "test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "mock-event-id-123"
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = SentryConfig {
        endpoint: format!("{}/api/1/store/", mock_server.uri()),
        client_token: Some("test-token".to_string()),
        environment: Some("production".to_string()),
        release: Some("0.2.49".to_string()),
        buffer_capacity: 100,
    };

    let client = SentryClient::new(config);

    let mut tags = HashMap::new();
    tags.insert("provider".to_string(), "deepseek".to_string());
    tags.insert("status_code".to_string(), "502".to_string());

    let event = RawEvent {
        platform: "rust".to_string(),
        release: Some("0.2.49".to_string()),
        environment: Some("production".to_string()),
        message: Some("UpstreamStatusError: 502 Bad Gateway".to_string()),
        exception: Some(Exception {
            error_type: "UpstreamStatusError".to_string(),
            value: Some("Upstream service 502".to_string()),
            stacktrace: Some(vec![Frame {
                filename: Some("crates/ponyllm-core/src/executor/mod.rs".to_string()),
                function: Some("execute_with_retry".to_string()),
                lineno: Some(120),
                colno: Some(10),
                in_app: Some(true),
            }]),
        }),
        tags: Some(tags),
        extra: Some(serde_json::json!({
            "model": "deepseek-chat",
            "attempt": 2
        })),
        breadcrumbs: None,
    };

    client.capture_event(event);

    // Wait slightly for background worker processing
    tokio::time::sleep(Duration::from_millis(150)).await;

    // MockServer's drop will verify expectations or we can explicitly verify received requests
    let received = mock_server.received_requests().await.unwrap();
    assert_eq!(
        received.len(),
        1,
        "Should have received exactly 1 sentry event"
    );

    let req_body: Value = serde_json::from_slice(&received[0].body).expect("Valid JSON");
    assert_eq!(req_body["message"], "UpstreamStatusError: 502 Bad Gateway");
    assert_eq!(req_body["platform"], "rust");
    assert_eq!(req_body["release"], "0.2.49");
    assert_eq!(req_body["environment"], "production");
    assert_eq!(req_body["exception"]["error_type"], "UpstreamStatusError");
    assert_eq!(req_body["tags"]["provider"], "deepseek");
    assert_eq!(req_body["extra"]["model"], "deepseek-chat");
}

#[tokio::test]
async fn test_sentry_non_blocking_and_circuit_breaking_on_outage() {
    let mock_server = MockServer::start().await;

    // Simulate Sentry server error (500) and slow/timeout delay
    Mock::given(method("POST"))
        .and(path("/api/1/store/"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_delay(Duration::from_millis(500))
                .set_body_string("Internal Server Error"),
        )
        .mount(&mock_server)
        .await;

    let config = SentryConfig {
        endpoint: format!("{}/api/1/store/", mock_server.uri()),
        client_token: Some("test-token".to_string()),
        environment: Some("production".to_string()),
        release: Some("0.2.49".to_string()),
        buffer_capacity: 10,
    };

    let client = SentryClient::new(config);

    let start = Instant::now();

    // Fire 20 events into a capacity-10 buffer while server is slow/erroring
    for i in 0..20 {
        client.capture_error(
            "RateLimitExceeded",
            &format!("Upstream 429 received: round {}", i),
            None,
            None,
        );
    }

    let elapsed = start.elapsed();
    // capture_error MUST NOT block caller thread (must return sub-millisecond or few milliseconds)
    assert!(
        elapsed < Duration::from_millis(50),
        "capture_error took {:?}, must be completely non-blocking",
        elapsed
    );

    // Let the worker process in background without crashing or blocking main tasks
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn test_sentry_sensitive_data_sanitization() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/1/store/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let config = SentryConfig {
        endpoint: format!("{}/api/1/store/", mock_server.uri()),
        client_token: Some("token-xyz".to_string()),
        environment: Some("production".to_string()),
        release: Some("0.2.49".to_string()),
        buffer_capacity: 50,
    };

    let client = SentryClient::new(config);

    let mut tags = HashMap::new();
    tags.insert(
        "api_key".to_string(),
        "sk-ant-api03-secretkey123456789".to_string(),
    );
    tags.insert("normal_tag".to_string(), "safe_value".to_string());

    let extra = serde_json::json!({
        "authorization": "Bearer sk-proj-supersecrettoken99999",
        "nested": {
            "token": "ghp_sensitivepersonaltoken",
            "x-api-key": "secret_key_value",
            "client_secret": "my_client_secret",
            "regular_field": "public_data"
        },
        "query": "model=deepseek&token=sk-987654321"
    });

    let event = RawEvent {
        platform: "rust".to_string(),
        release: Some("0.2.49".to_string()),
        environment: Some("test".to_string()),
        message: Some("Failed request with Bearer sk-1234567890abcdef".to_string()),
        exception: Some(Exception {
            error_type: "AuthInvalid".to_string(),
            value: Some("Invalid token: sk-ant-api03-abcdef123456".to_string()),
            stacktrace: None,
        }),
        tags: Some(tags),
        extra: Some(extra),
        breadcrumbs: None,
    };

    client.capture_event(event);

    tokio::time::sleep(Duration::from_millis(150)).await;

    let received = mock_server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);

    let body_str = String::from_utf8_lossy(&received[0].body);

    // Verify secret patterns are sanitized and not leaked
    assert!(
        !body_str.contains("sk-ant-api03-secretkey123456789"),
        "Raw API key in tags was leaked"
    );
    assert!(
        !body_str.contains("sk-proj-supersecrettoken99999"),
        "Bearer token in extra was leaked"
    );
    assert!(
        !body_str.contains("ghp_sensitivepersonaltoken"),
        "Nested token was leaked"
    );
    assert!(
        !body_str.contains("secret_key_value"),
        "Nested x-api-key was leaked"
    );
    assert!(
        !body_str.contains("my_client_secret"),
        "client_secret was leaked"
    );

    let req_body: Value = serde_json::from_str(&body_str).expect("Valid JSON");
    assert_eq!(req_body["tags"]["normal_tag"], "safe_value");
    assert_eq!(req_body["extra"]["nested"]["regular_field"], "public_data");
    assert_eq!(req_body["tags"]["api_key"], "[REDACTED_API_KEY]");
    assert_eq!(req_body["extra"]["authorization"], "[REDACTED]");
    assert_eq!(req_body["extra"]["nested"]["token"], "[REDACTED]");
    assert_eq!(req_body["extra"]["nested"]["x-api-key"], "[REDACTED]");
}
