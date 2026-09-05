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
async fn test_thinking_scrubbing_for_non_reasoning_models() {
    let captured_requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let captured_clone = captured_requests.clone();

    // Mock upstream server that inspects received payload
    let mock_upstream = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(req): Json<serde_json::Value>| {
            let cap = captured_clone.clone();
            async move {
                cap.lock().push(req.clone());
                axum::Json(json!({
                    "id": "chatcmpl-test-123",
                    "object": "chat.completion",
                    "created": 1710000000,
                    "model": "gpt-4o",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "Hello!"
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 5,
                        "completion_tokens": 5,
                        "total_tokens": 10
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

    let pool = Arc::new(KeyPool::new("openai", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-mock-key-123", 1, 10));

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", upstream_addr),
            default_model: "gpt-4o".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 2.5,
            cached_price: 1.25,
            output_price: 10.0,
            models: vec!["gpt-4o".to_string()],
            model_specs: vec![ModelSpec {
                name: "gpt-4o".to_string(),
                tier: ModelTier::Standard,
                // gpt-4o defaults to Off, max Off
                ..Default::default()
            }],
            default_protocol: Some(UpstreamProtocol::Chat),
            chat_url: None,
            responses_url: None,
            messages_url: None,
        },
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("openai", pool);

    let gateway_app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // 1. Send request with reasoning_effort in body
    let resp1 = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi"}],
            "reasoning_effort": "high"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);

    // 2. Send request with thinking in model name suffix
    let resp2 = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "gpt-4o:high",
            "messages": [{"role": "user", "content": "Hi again"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);

    // 3. Send request with X-Pony-Thinking header
    let resp3 = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .header("X-Pony-Thinking", "high")
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi header"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp3.status(), 200);

    // Verify all captured upstream payloads have reasoning_effort and thinking removed!
    let reqs = captured_requests.lock().clone();
    assert_eq!(reqs.len(), 3);
    for (i, req) in reqs.iter().enumerate() {
        assert!(
            req.get("reasoning_effort").is_none() || req["reasoning_effort"].is_null(),
            "Request {} should not have reasoning_effort: {:?}",
            i,
            req
        );
        assert!(
            req.get("thinking").is_none() || req["thinking"].is_null(),
            "Request {} should not have thinking: {:?}",
            i,
            req
        );
        assert!(
            req.get("reasoning").is_none() || req["reasoning"].is_null(),
            "Request {} should not have reasoning: {:?}",
            i,
            req
        );
    }
}

#[tokio::test]
async fn test_thinking_forwarding_and_clamping_for_reasoning_models() {
    let captured_requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let captured_clone = captured_requests.clone();

    let mock_upstream = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(req): Json<serde_json::Value>| {
            let cap = captured_clone.clone();
            async move {
                cap.lock().push(req.clone());
                axum::Json(json!({
                    "id": "chatcmpl-test-456",
                    "object": "chat.completion",
                    "created": 1710000000,
                    "model": "o3-mini",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "Reasoned response"
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 5,
                        "completion_tokens": 5,
                        "total_tokens": 10
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

    let pool = Arc::new(KeyPool::new("openai", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-mock-key-123", 1, 10));

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", upstream_addr),
            default_model: "o3-mini".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 1.1,
            cached_price: 0.55,
            output_price: 4.4,
            models: vec!["o3-mini".to_string()],
            model_specs: vec![ModelSpec {
                name: "o3-mini".to_string(),
                tier: ModelTier::Standard,
                thinking_default: Some(ReasoningEffort::Medium),
                thinking_max: Some(ReasoningEffort::Medium), // Ceiling clamped to Medium!
                ..Default::default()
            }],
            default_protocol: Some(UpstreamProtocol::Chat),
            chat_url: None,
            responses_url: None,
            messages_url: None,
        },
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("openai", pool);

    let gateway_app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // 1. Request with High effort -> should be clamped to Medium
    let resp1 = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "o3-mini",
            "messages": [{"role": "user", "content": "Solve math"}],
            "reasoning_effort": "high"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);

    // 2. Request with Low effort -> should remain Low (since Low <= Medium)
    let resp2 = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "o3-mini",
            "messages": [{"role": "user", "content": "Solve easy math"}],
            "reasoning_effort": "low"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);

    // 3. Request with no effort specified -> should fallback to default (Medium)
    let resp3 = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "o3-mini",
            "messages": [{"role": "user", "content": "Solve default"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp3.status(), 200);

    let reqs = captured_requests.lock().clone();
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[0]["reasoning_effort"], "medium"); // clamped from high
    assert_eq!(reqs[1]["reasoning_effort"], "low");    // preserved low
    assert_eq!(reqs[2]["reasoning_effort"], "medium"); // fallback to default
}

#[tokio::test]
async fn test_cross_protocol_thinking_translation() {
    let captured_requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let captured_clone = captured_requests.clone();

    // Mock Anthropic upstream server
    let mock_anthropic = Router::new().route(
        "/v1/messages",
        post(move |Json(req): Json<serde_json::Value>| {
            let cap = captured_clone.clone();
            async move {
                cap.lock().push(req.clone());
                axum::Json(json!({
                    "id": "msg-mock-123",
                    "type": "message",
                    "role": "assistant",
                    "content": [{
                        "type": "text",
                        "text": "Hello Anthropic"
                    }],
                    "model": "claude-opus-5",
                    "stop_reason": "end_turn",
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 10
                    }
                }))
            }
        }),
    );

    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock_anthropic).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("anthropic", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-ant-mock", 1, 10));

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "anthropic".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", upstream_addr),
            default_model: "claude-opus-5".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 15.0,
            cached_price: 1.5,
            output_price: 75.0,
            models: vec!["claude-opus-5".to_string()],
            model_specs: vec![ModelSpec {
                name: "claude-opus-5".to_string(),
                tier: ModelTier::Flagship,
                thinking_default: Some(ReasoningEffort::Medium),
                thinking_max: Some(ReasoningEffort::High),
                ..Default::default()
            }],
            default_protocol: Some(UpstreamProtocol::Anthropic),
            chat_url: None,
            responses_url: None,
            messages_url: None,
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

    // 1. Call OpenAI Chat endpoint, routed to Anthropic upstream with reasoning_effort
    let resp1 = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "claude-opus-5",
            "messages": [{"role": "user", "content": "Explain quantum computing"}],
            "reasoning_effort": "high"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);

    let reqs = captured_requests.lock().clone();
    assert_eq!(reqs.len(), 1);
    let ant_req = &reqs[0];

    // Check that Anthropic upstream received thinking object with effort: high
    assert_eq!(ant_req["thinking"]["type"], "enabled");
    assert_eq!(ant_req["thinking"]["effort"], "high");
    assert_eq!(ant_req["reasoning_effort"], "high");
}

#[tokio::test]
async fn test_thinking_precedence_header_wins() {
    let captured_requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let captured_clone = captured_requests.clone();

    let mock_upstream = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(req): Json<serde_json::Value>| {
            let cap = captured_clone.clone();
            async move {
                cap.lock().push(req.clone());
                axum::Json(json!({
                    "id": "chatcmpl-test-789",
                    "object": "chat.completion",
                    "created": 1710000000,
                    "model": "deepseek-reasoner",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "Reasoned"
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 5,
                        "completion_tokens": 5,
                        "total_tokens": 10
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
    pool.add_key(ApiKeyEntry::new("k1", "sk-ds-mock", 1, 10));

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "deepseek".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", upstream_addr),
            default_model: "deepseek-reasoner".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.55,
            cached_price: 0.14,
            output_price: 2.19,
            models: vec!["deepseek-reasoner".to_string()],
            model_specs: vec![ModelSpec {
                name: "deepseek-reasoner".to_string(),
                tier: ModelTier::Flagship,
                thinking_default: Some(ReasoningEffort::Medium),
                thinking_max: Some(ReasoningEffort::High),
                ..Default::default()
            }],
            default_protocol: Some(UpstreamProtocol::Chat),
            chat_url: None,
            responses_url: None,
            messages_url: None,
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

    // Body says "low", model says ":medium", header says "high"
    // Header MUST win -> "high"
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .header("X-Pony-Thinking", "high")
        .json(&json!({
            "model": "deepseek-reasoner:medium",
            "messages": [{"role": "user", "content": "Precedence test"}],
            "reasoning_effort": "low"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let reqs = captured_requests.lock().clone();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0]["reasoning_effort"], "high");
}
