#![allow(clippy::field_reassign_with_default)]

use std::sync::Arc;
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use serde_json::json;
use ponyllm_core::pool::*;
use ponyllm_protocol::common::ReasoningEffort;
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};

#[tokio::test]
async fn test_thinking_output_safeguard_chat_flooring() {
    let captured_requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let captured_clone = captured_requests.clone();

    let mock_upstream = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(req): Json<serde_json::Value>| {
            let cap = captured_clone.clone();
            async move {
                cap.lock().push(req.clone());
                axum::Json(json!({
                    "id": "chatcmpl-test-safeguard",
                    "object": "chat.completion",
                    "created": 1710000000,
                    "model": "deepseek-reasoner",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "Reasoned response",
                            "reasoning_content": "Detailed thinking steps"
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 100,
                        "total_tokens": 110
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

    let pool = Arc::new(KeyPool::new("deepseek", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-mock-key-123", 1, 10));

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "deepseek".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", upstream_addr),
            default_model: "deepseek-reasoner".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.5,
            cached_price: 0.25,
            output_price: 1.0,
            models: vec!["deepseek-reasoner".to_string(), "deepseek-chat".to_string()],
            model_specs: vec![
                ModelSpec {
                    name: "deepseek-reasoner".to_string(),
                    tier: ModelTier::Flagship,
                    max_output: "32K".to_string(),
                    thinking_default: Some(ReasoningEffort::High),
                    thinking_max: Some(ReasoningEffort::High),
                    ..Default::default()
                },
                ModelSpec {
                    name: "deepseek-chat".to_string(),
                    tier: ModelTier::Standard,
                    max_output: "8K".to_string(),
                    thinking_default: Some(ReasoningEffort::Off),
                    thinking_max: Some(ReasoningEffort::Off),
                    ..Default::default()
                },
            ],
            default_protocol: Some(UpstreamProtocol::Chat),
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
        },
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek", pool);

    let gateway_app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // 1. Send request with tiny max_tokens (2048) to High-thinking model
    // Safeguard must elevate max_tokens to at least 16384 (or model max) to avoid choking
    let resp1 = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "deepseek-reasoner",
            "messages": [{"role": "user", "content": "Complex logic puzzle"}],
            "max_tokens": 2048
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);

    let reqs = captured_requests.lock().clone();
    assert_eq!(reqs.len(), 1);
    let forwarded_max = reqs[0]["max_tokens"].as_u64().unwrap();
    assert!(
        forwarded_max >= 16384,
        "High thinking model with 2048 max_tokens must be floored to >= 16384, got {}",
        forwarded_max
    );

    // 2. Send request to non-thinking model with max_tokens (2048)
    // Non-thinking model should preserve client's 2048
    let resp2 = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "deepseek-chat",
            "messages": [{"role": "user", "content": "Quick greeting"}],
            "max_tokens": 2048
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);

    let reqs = captured_requests.lock().clone();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1]["max_tokens"], 2048, "Non-thinking model must not be floored");
}

#[tokio::test]
async fn test_thinking_output_safeguard_messages_flooring() {
    let captured_requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let captured_clone = captured_requests.clone();

    let mock_upstream = Router::new().route(
        "/v1/messages",
        post(move |Json(req): Json<serde_json::Value>| {
            let cap = captured_clone.clone();
            async move {
                cap.lock().push(req.clone());
                axum::Json(json!({
                    "id": "msg-test-safeguard",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-3-7-sonnet",
                    "content": [{
                        "type": "text",
                        "text": "Answer"
                    }],
                    "stop_reason": "end_turn",
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 50
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

    let pool = Arc::new(KeyPool::new("anthropic", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-ant-mock", 1, 10));

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "anthropic".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", upstream_addr),
            default_model: "claude-3-7-sonnet".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 3.0,
            cached_price: 1.5,
            output_price: 15.0,
            models: vec!["claude-3-7-sonnet".to_string()],
            model_specs: vec![
                ModelSpec {
                    name: "claude-3-7-sonnet".to_string(),
                    tier: ModelTier::Flagship,
                    max_output: "64K".to_string(),
                    thinking_default: Some(ReasoningEffort::High),
                    thinking_max: Some(ReasoningEffort::High),
                    ..Default::default()
                },
            ],
            default_protocol: Some(UpstreamProtocol::Anthropic),
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
        },
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("anthropic", pool);

    let gateway_app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // Client passes max_tokens: 4096 to Messages API for high thinking model
    let resp = client
        .post(format!("http://{}/v1/messages", gateway_addr))
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "messages": [{"role": "user", "content": "Prove P vs NP"}],
            "max_tokens": 4096
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let reqs = captured_requests.lock().clone();
    assert_eq!(reqs.len(), 1);
    let forwarded_max = reqs[0]["max_tokens"].as_u64().unwrap();
    assert!(
        forwarded_max >= 16384,
        "Messages max_tokens must be floored to >= 16384 for High thinking, got {}",
        forwarded_max
    );
}
