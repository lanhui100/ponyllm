use std::sync::Arc;
use std::time::Instant;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ponyllm_core::telemetry::{EventCtx, GatewayEvent};
use ponyllm_server::app::create_app;
use ponyllm_server::state::AppState;
use ponyllm_server::GatewayConfig;
use serde_json::Value;

#[tokio::test]
async fn test_telemetry_history_and_stream_uptime_bars() {
    let mut config = GatewayConfig::default();
    config.api_key = "test-token".to_string();
    let state = Arc::new(AppState::new(config));

    // Emit a test event through the event bus
    let mut ctx = EventCtx::new("req-hist-1", "/v1/chat/completions", Instant::now());
    ctx.model = Some("deepseek-chat".to_string());
    state.emit(
        &ctx,
        Some("deepseek".to_string()),
        GatewayEvent::RequestCompleted {
            status_code: 200,
            latency_ms: 150.0,
            prompt_tokens: 120,
            completion_tokens: 80,
            tps: Some(50.0),
            request_snippet: None,
            response_snippet: None,
        },
    );

    // Also record connectivity sample for deepseek and gateway
    let now_ms = 1_700_000_000_000u64;
    state
        .connectivity_sampler
        .record("deepseek", now_ms, Some(150.0), true);
    state
        .connectivity_sampler
        .record("gateway", now_ms, Some(2.5), true);

    let app = create_app(state.clone());

    // 1. Test GET /v1/telemetry/history?range=24h
    let req = Request::builder()
        .uri("/v1/telemetry/history?range=24h")
        .header("authorization", "Bearer test-token")
        .body(Body::empty())
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["range"], "24h");
    assert!(json["points"].is_array());
    assert_eq!(json["total_tokens"], 200);
    assert_eq!(json["provider_tokens"]["deepseek"], 200);
    assert_eq!(json["model_tokens"]["deepseek-chat"], 200);

    // 2. Test GET /v1/telemetry/stream includes gateway_uptime_bars and provider uptime_bars
    let req2 = Request::builder()
        .uri("/v1/telemetry/stream")
        .header("authorization", "Bearer test-token")
        .body(Body::empty())
        .unwrap();

    let resp2 = tower::ServiceExt::oneshot(app.clone(), req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let bytes2 = axum::body::to_bytes(resp2.into_body(), usize::MAX).await.unwrap();
    let json2: Value = serde_json::from_slice(&bytes2).unwrap();
    assert!(json2["gateway_uptime_bars"]["slots"].is_array());
    assert_eq!(json2["gateway_uptime_bars"]["slots"].as_array().unwrap().len(), 24);
    // Non-streaming provider deepseek should appear in providers map
    assert!(json2["providers"]["deepseek"].is_object(), "deepseek provider should be in stream snapshot");
    assert_eq!(json2["providers"]["deepseek"]["uptime_bars"]["slots"].as_array().unwrap().len(), 40);

    // 3. Test GET /v1/telemetry/history with invalid range returns 400
    let req3 = Request::builder()
        .uri("/v1/telemetry/history?range=invalid_range")
        .header("authorization", "Bearer test-token")
        .body(Body::empty())
        .unwrap();

    let resp3 = tower::ServiceExt::oneshot(app, req3).await.unwrap();
    assert_eq!(resp3.status(), StatusCode::BAD_REQUEST);
}
