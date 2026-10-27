//! Acceptance & Red-Phase Tests for `thinking_efforts` wire mapping
//!
//! Contract:
//! 1. Custom string mapping overrides upstream reasoning_effort (e.g., "high" -> "xhigh", "max" -> "ultra").
//! 2. Off with empty string suppresses reasoning_effort parameter.
//! 3. Off with non-empty string sends explicit string (e.g. "none").
//! 4. Admin API validates that keys are within [off, low, medium, high, max] and rejects unknown tiers.
//! 5. Adversarial checks: empty non-off values or invalid characters rejected with 400.

use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use ponyllm_core::pool::*;
use ponyllm_protocol::common::ReasoningEffort;
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

#[tokio::test]
async fn test_thinking_efforts_wire_mapping_chat_completions() {
    let captured_requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let captured_clone = captured_requests.clone();

    let mock_upstream = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(req): Json<serde_json::Value>| {
            let cap = captured_clone.clone();
            async move {
                cap.lock().push(req.clone());
                axum::Json(json!({
                    "id": "chatcmpl-test-custom",
                    "object": "chat.completion",
                    "created": 1710000000,
                    "model": "custom-reasoner",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "Mapped successfully"
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 10,
                        "total_tokens": 20
                    }
                }))
            }
        }),
    );

    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock_upstream).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("custom-provider", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test-key", 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;

    let mut efforts = BTreeMap::new();
    efforts.insert("high".to_string(), "xhigh".to_string());
    efforts.insert("max".to_string(), "ultra".to_string());
    efforts.insert("low".to_string(), "minimal".to_string());
    efforts.insert("off".to_string(), "".to_string());

    let mut model_spec = ModelSpec {
        name: "custom-reasoner".to_string(),
        tier: ModelTier::Standard,
        thinking_default: Some(ReasoningEffort::High),
        thinking_max: Some(ReasoningEffort::Max),
        ..Default::default()
    };
    model_spec.thinking_efforts = efforts;

    config.providers.insert(
        "custom-provider".to_string(),
        ProviderConfig {
            rate_limits: None,
            base_url: format!("http://{}", upstream_addr),
            default_model: "custom-reasoner".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 1.0,
            cached_price: 0.5,
            output_price: 2.0,
            models: vec!["custom-reasoner".to_string()],
            model_specs: vec![model_spec],
            default_protocol: Some(UpstreamProtocol::Chat),
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: Some(30),
            ttfb_timeout_secs: None,
            egress_pool: Vec::new(),
            egress_strategy: "round_robin".to_string(),
        },
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("custom-provider", pool);
    let gateway_app = create_app(state);

    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // Case 1: Client sends "high" -> Upstream must receive "xhigh"
    let req1 = json!({
        "model": "custom-reasoner",
        "messages": [{"role": "user", "content": "hello"}],
        "reasoning_effort": "high"
    });
    let resp1 = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&req1)
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);

    let captured = captured_requests.lock().clone();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0]["reasoning_effort"], "xhigh");

    // Case 2: Client sends "off" -> Wire mapping is "" -> parameter suppressed
    let req2 = json!({
        "model": "custom-reasoner",
        "messages": [{"role": "user", "content": "hello"}],
        "reasoning_effort": "off"
    });
    let resp2 = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&req2)
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);

    let captured2 = captured_requests.lock().clone();
    assert_eq!(captured2.len(), 2);
    assert!(captured2[1].get("reasoning_effort").is_none() || captured2[1]["reasoning_effort"].is_null());
}
