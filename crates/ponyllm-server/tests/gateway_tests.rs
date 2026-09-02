use std::sync::Arc;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::json;
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};

#[tokio::test]
async fn test_gateway_chat_and_messages_endpoints() {
    // 1. Mock upstream server that handles OpenAI Chat format
    let mock_upstream = Router::new().route(
        "/v1/chat/completions",
        post(|Json(req): Json<serde_json::Value>| async move {
            let user_text = req["messages"][0]["content"].as_str().unwrap_or_default();
            axum::Json(json!({
                "id": "chatcmpl-mock-123",
                "object": "chat.completion",
                "created": 1710000000,
                "model": "gpt-4o",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": format!("Echo: {}", user_text),
                        "reasoning_content": "Processed request"
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 15,
                    "total_tokens": 25
                }
            }))
        }),
    );

    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock_upstream).await.unwrap();
    });

    // 2. Setup ponyllm gateway state
    let pool = Arc::new(KeyPool::new("openai", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-mock-key-123456", 1, 10));

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", upstream_addr),
            default_model: "gpt-4o".to_string(),
            models: Vec::new(),
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

    // 3. Test Health endpoint
    let health_resp = client
        .get(format!("http://{}/health", gateway_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(health_resp.status(), 200);

    // 4. Test OpenAI Chat endpoint (/v1/chat/completions)
    let chat_resp = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hello from Chat"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(chat_resp.status(), 200);
    let chat_body: serde_json::Value = chat_resp.json().await.unwrap();
    assert_eq!(chat_body["choices"][0]["message"]["content"], "Echo: Hello from Chat");

    // 5. Test Anthropic Messages endpoint (/v1/messages) translated to OpenAI upstream!
    let ant_resp = client
        .post(format!("http://{}/v1/messages", gateway_addr))
        .json(&json!({
            "model": "gpt-4o",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "Hello from Claude Client"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(ant_resp.status(), 200);
    let ant_body: serde_json::Value = ant_resp.json().await.unwrap();
    assert_eq!(ant_body["type"], "message");
    assert_eq!(ant_body["role"], "assistant");
    assert_eq!(ant_body["content"][0]["type"], "thinking");
    assert_eq!(ant_body["content"][1]["type"], "text");
    assert_eq!(ant_body["content"][1]["text"], "Echo: Hello from Claude Client");

    // 6. Test Telemetry / Flight recorder endpoint
    let recorder_resp = client
        .get(format!("http://{}/v1/telemetry/recorder", gateway_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(recorder_resp.status(), 200);
    let recorder_body: serde_json::Value = recorder_resp.json().await.unwrap();
    assert!(recorder_body.as_array().unwrap().len() >= 2);
}

#[tokio::test]
async fn test_multi_provider_dynamic_model_routing() {
    let mut config = GatewayConfig::default();
    config.providers.insert(
        "deepseek".to_string(),
        ProviderConfig {
            base_url: "https://api.deepseek.com".to_string(),
            default_model: "deepseek-v4-flash".to_string(),
            models: Vec::new(),
        },
    );
    config.providers.insert(
        "deepseek-anthropic".to_string(),
        ProviderConfig {
            base_url: "https://api.deepseek.com/anthropic".to_string(),
            default_model: "deepseek-v4-flash".to_string(),
            models: Vec::new(),
        },
    );
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            base_url: "https://api.openai.com".to_string(),
            default_model: "gpt-4o".to_string(),
            models: Vec::new(),
        },
    );
    config.providers.insert(
        "anthropic".to_string(),
        ProviderConfig {
            base_url: "https://api.anthropic.com".to_string(),
            default_model: "claude-3-7-sonnet-20250219".to_string(),
            models: Vec::new(),
        },
    );

    let state = AppState::new(config);

    // 1. Exact default model match
    let (prov_ds, _) = state.resolve_provider("deepseek-v4-flash").unwrap();
    assert!(prov_ds.starts_with("deepseek"));

    let (prov_ant, _) = state.resolve_provider("claude-3-7-sonnet-20250219").unwrap();
    assert_eq!(prov_ant, "anthropic");

    // 2. Prefix slash match
    let (prov_pref, _) = state.resolve_provider("openai/gpt-3.5-turbo").unwrap();
    assert_eq!(prov_pref, "openai");

    let (prov_ds_ant, _) = state.resolve_provider("deepseek-anthropic/deepseek-v4-flash").unwrap();
    assert_eq!(prov_ds_ant, "deepseek-anthropic");

    // 3. Keyword heuristic match
    let (prov_gpt, _) = state.resolve_provider("gpt-4o-mini").unwrap();
    assert_eq!(prov_gpt, "openai");

    let (prov_cl, _) = state.resolve_provider("claude-3-5-sonnet").unwrap();
    assert_eq!(prov_cl, "anthropic");
}

#[tokio::test]
async fn test_anthropic_upstream_direct_and_cross_routing() {
    // 1. Mock upstream server that handles native Anthropic format at /v1/messages
    let mock_anthropic_upstream = Router::new().route(
        "/anthropic/v1/messages",
        post(|Json(req): Json<serde_json::Value>| async move {
            let user_text = req["messages"][0]["content"].as_str().unwrap_or_default();
            axum::Json(json!({
                "id": "msg_mock_999",
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "text",
                    "text": format!("Anthropic Echo: {}", user_text)
                }],
                "model": "deepseek-v4-flash",
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {
                    "input_tokens": 12,
                    "output_tokens": 18
                }
            }))
        }),
    );

    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock_anthropic_upstream).await.unwrap();
    });

    // 2. Setup gateway pointing to deepseek-anthropic base_url (ends with /anthropic)
    let pool = Arc::new(KeyPool::new("deepseek-anthropic", RoutingStrategy::Priority));
    pool.add_key(ApiKeyEntry::new("ds-key-1", "sk-ds-secret-123456", 1, 10));

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "deepseek-anthropic".to_string(),
        ProviderConfig {
            base_url: format!("http://{}/anthropic", upstream_addr),
            default_model: "deepseek-v4-flash".to_string(),
            models: Vec::new(),
        },
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek-anthropic", pool);

    let gateway_app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // 3. Test Anthropic client requesting /v1/messages -> direct passthrough to Anthropic upstream!
    let ant_resp = client
        .post(format!("http://{}/v1/messages", gateway_addr))
        .json(&json!({
            "model": "deepseek-anthropic/deepseek-v4-flash",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "Direct Anthropic Test"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(ant_resp.status(), 200);
    let ant_body: serde_json::Value = ant_resp.json().await.unwrap();
    assert_eq!(ant_body["type"], "message");
    assert_eq!(ant_body["content"][0]["text"], "Anthropic Echo: Direct Anthropic Test");

    // 4. Test OpenAI Chat client requesting /v1/chat/completions -> translated to Anthropic upstream and back!
    let chat_resp = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "deepseek-anthropic/deepseek-v4-flash",
            "messages": [{"role": "user", "content": "Cross Chat Test"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(chat_resp.status(), 200);
    let chat_body: serde_json::Value = chat_resp.json().await.unwrap();
    assert_eq!(chat_body["choices"][0]["message"]["content"], "Anthropic Echo: Cross Chat Test");
}

#[tokio::test]
async fn test_gateway_models_endpoints() {
    let mut config = GatewayConfig::default();
    config.providers.insert(
        "deepseek".to_string(),
        ProviderConfig {
            base_url: "https://api.deepseek.com".to_string(),
            default_model: "deepseek-v4-flash".to_string(),
            models: vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()],
        },
    );
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            base_url: "https://api.openai.com".to_string(),
            default_model: "gpt-4o".to_string(),
            models: vec!["gpt-4o-mini".to_string()],
        },
    );

    let state = Arc::new(AppState::new(config));
    let gateway_app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // 1. Test GET /v1/models
    let models_resp = client
        .get(format!("http://{}/v1/models", gateway_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(models_resp.status(), 200);
    let models_body: serde_json::Value = models_resp.json().await.unwrap();
    assert_eq!(models_body["object"], "list");
    let list = models_body["data"].as_array().unwrap();
    assert_eq!(list.len(), 5); // deepseek-v4-flash, deepseek-chat, deepseek-reasoner, gpt-4o, gpt-4o-mini

    // 2. Test GET /v1/models/:model_id for existing model
    let model_resp = client
        .get(format!("http://{}/v1/models/deepseek-chat", gateway_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(model_resp.status(), 200);
    let model_body: serde_json::Value = model_resp.json().await.unwrap();
    assert_eq!(model_body["id"], "deepseek-chat");
    assert_eq!(model_body["owned_by"], "deepseek");

    // 3. Test GET /v1/models/:model_id for non-existent model (404)
    let not_found_resp = client
        .get(format!("http://{}/v1/models/non-existent-model", gateway_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(not_found_resp.status(), 404);
}
