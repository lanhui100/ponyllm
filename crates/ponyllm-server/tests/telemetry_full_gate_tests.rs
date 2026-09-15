//! H3 regression: full recorder frames require admin writes enabled.
//!
//! - `GET /v1/telemetry/recorder?full=true` with writes OFF -> 404
//!   `telemetry_full_disabled` (no prompt bulk-read with a plain token).
//! - `GET /v1/telemetry/recorder/{id}` with writes OFF -> same 404.
//! - Summaries (`?full` unset) and aggregates (metrics/stream/history)
//!   stay available under the normal gateway token in both modes.
//! - With writes ON, `?full=true` and single-frame reads keep working.

use std::sync::Arc;
use std::time::Duration;
use axum::http::StatusCode;
use ponyllm_core::telemetry::{FlightFrame, FlightRecorder};
use ponyllm_server::app::create_app;
use ponyllm_server::state::AppState;
use ponyllm_server::GatewayConfig;
use serde_json::Value;

fn test_frame(id: &str) -> FlightFrame {
    FlightFrame {
        request_id: id.to_string(),
        endpoint: "/v1/chat/completions".to_string(),
        provider: Some("openai".to_string()),
        key_id: "k1".to_string(),
        raw_key: None,
        attempt: Some(0),
        status_code: Some(200),
        latency: Duration::from_millis(50),
        error: None,
        request_snippet: Some("{\"model\":\"gpt-4o\",\"messages\":[\"hello\"]}".to_string()),
        response_snippet: Some("{\"text\":\"world\"}".to_string()),
        prompt_tokens: None,
        completion_tokens: None,
        cached_tokens: None,
        ttft_ms: None,
        downstream_ttft_ms: None,
        stream_flow: None,
    }
}

async fn spawn_with_write(admin_write_enabled: bool) -> (String, String) {
    let mut config = GatewayConfig::default();
    config.api_key = "test-token".to_string();
    config.admin_write_enabled = admin_write_enabled;
    let state = Arc::new(AppState::new(config));
    state.flight_recorder.record(test_frame("req-h3-1"));
    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), "test-token".to_string())
}

async fn get(base: &str, token: &str, path: &str) -> (StatusCode, Value) {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}{}", base, path))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let json: Value = resp.json().await.unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test]
async fn test_full_frames_require_admin_writes() {
    // Writes OFF (the default): full reads refused, summaries work.
    let (base, token) = spawn_with_write(false).await;

    let (st, body) = get(&base, &token, "/v1/telemetry/recorder?full=true").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "telemetry_full_disabled");

    let (st, body) = get(&base, &token, "/v1/telemetry/recorder/req-h3-1").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "telemetry_full_disabled");

    let (st, _) = get(&base, &token, "/v1/telemetry/recorder").await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = get(&base, &token, "/v1/telemetry/metrics").await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = get(&base, &token, "/v1/telemetry/stream").await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = get(&base, &token, "/v1/telemetry/history?range=24h").await;
    assert_eq!(st, StatusCode::OK);

    // Writes ON: full reads keep working.
    let (base, token) = spawn_with_write(true).await;
    let (st, body) = get(&base, &token, "/v1/telemetry/recorder?full=true").await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.as_array().unwrap().iter().any(|f| f["request_id"] == "req-h3-1"));

    let (st, body) = get(&base, &token, "/v1/telemetry/recorder/req-h3-1").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["request_id"], "req-h3-1");
    // Full text present when explicitly enabled.
    assert!(body["request_snippet"].as_str().unwrap().contains("hello"));

    // Summary mode never carries snippets, in either mode.
    let (st, body) = get(&base, &token, "/v1/telemetry/recorder").await;
    assert_eq!(st, StatusCode::OK);
    for frame in body.as_array().unwrap() {
        assert!(frame.get("request_snippet").is_none());
        assert!(frame.get("response_snippet").is_none());
    }
}

#[test]
fn recorder_summary_strips_snippets_unit() {
    // Belt-and-braces at the recorder level (no HTTP involved).
    let rec = FlightRecorder::new(4);
    rec.record(test_frame("u1"));
    let summaries = rec.get_recent_summaries();
    assert_eq!(summaries.len(), 1);
    assert!(summaries[0].request_snippet.is_none());
    assert!(summaries[0].response_snippet.is_none());
    assert!(rec.get_frame("u1").unwrap().request_snippet.is_some());
}
