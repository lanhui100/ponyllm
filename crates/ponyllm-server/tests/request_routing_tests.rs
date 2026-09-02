use std::sync::Arc;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::json;
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig, ModelSpec};
use ponyllm_server::routes::models::ParsedRequestModel;

#[test]
fn test_parsed_request_model_sanitizer() {
    // 1. Clean model without tags
    let p1 = ParsedRequestModel::parse("deepseek-v4-flash");
    assert_eq!(p1.raw_requested_model, "deepseek-v4-flash");
    assert_eq!(p1.clean_model_name, "deepseek-v4-flash");
    assert!(!p1.is_auto);
    assert!(!p1.is_1m_context);
    assert_eq!(p1.strategy_override, None);

    // 2. Model with [1m] and :economy strategy
    let p2 = ParsedRequestModel::parse("deepseek-v4-flash[1m]:economy");
    assert_eq!(p2.raw_requested_model, "deepseek-v4-flash[1m]:economy");
    assert_eq!(p2.clean_model_name, "deepseek-v4-flash");
    assert!(!p2.is_auto);
    assert!(p2.is_1m_context);
    assert_eq!(p2.strategy_override, Some(GatewayRoutingStrategy::Economy));

    // 3. Case insensitive [ 1M ] with space and :fastest
    let p3 = ParsedRequestModel::parse("claude-3-7-sonnet [ 1M ] : fastest");
    assert_eq!(p3.clean_model_name, "claude-3-7-sonnet");
    assert!(p3.is_1m_context);
    assert_eq!(p3.strategy_override, Some(GatewayRoutingStrategy::Speed));

    // 4. Model with colon tags (e.g. Docker / Ollama style tags like llama3:70b:speed)
    let p4_tagged = ParsedRequestModel::parse("meta-llama/llama-3:70b:speed");
    assert_eq!(p4_tagged.clean_model_name, "meta-llama/llama-3:70b");
    assert_eq!(p4_tagged.strategy_override, Some(GatewayRoutingStrategy::Speed));

    // 5. Auto virtual model: default tier is Standard, supports explicit :flagship
    let p4 = ParsedRequestModel::parse("auto");
    assert_eq!(p4.clean_model_name, "auto");
    assert!(p4.is_auto);
    assert_eq!(p4.explicit_tier, None); // default resolves to Standard

    let p5 = ParsedRequestModel::parse("auto:flagship:economy");
    assert!(p5.is_auto);
    assert_eq!(p5.clean_model_name, "auto");
    assert_eq!(p5.explicit_tier, Some(ModelTier::Flagship));
    assert_eq!(p5.strategy_override, Some(GatewayRoutingStrategy::Economy));

    let p6 = ParsedRequestModel::parse("auto[1m]:speed");
    assert!(p6.is_auto);
    assert!(p6.is_1m_context);
    assert_eq!(p6.strategy_override, Some(GatewayRoutingStrategy::Speed));
}

#[tokio::test]
async fn test_model_echo_policy_and_auto_routing() {
    // 1. Mock upstream server
    let mock_upstream = Router::new().route(
        "/v1/chat/completions",
        post(|Json(req): Json<serde_json::Value>| async move {
            let upstream_model = req["model"].as_str().unwrap_or_default().to_string();
            assert!(!upstream_model.contains("[1m]"));
            assert!(!upstream_model.contains(":"));

            axum::Json(json!({
                "id": "chatcmpl-mock-456",
                "object": "chat.completion",
                "created": 1710000000,
                "model": upstream_model,
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hello from upstream"
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 50,
                    "completion_tokens": 20,
                    "total_tokens": 70,
                    "prompt_tokens_details": { "cached_tokens": 30 }
                }
            }))
        }),
    );

    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock_upstream).await.unwrap();
    });

    // 2. Setup gateway with Flagship (1M) and Standard (128K) models
    let pool_ds = Arc::new(KeyPool::new("deepseek", RoutingStrategy::RoundRobin));
    pool_ds.add_key(ApiKeyEntry::new("ds-k1", "sk-ds-key", 1, 10));

    let pool_openai = Arc::new(KeyPool::new("openai", RoutingStrategy::RoundRobin));
    pool_openai.add_key(ApiKeyEntry::new("oa-k1", "sk-oa-key", 1, 10));

    let mut config = GatewayConfig::default();
    config.default_strategy = GatewayRoutingStrategy::Economy;

    config.providers.insert(
        "deepseek".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", upstream_addr),
            default_model: "deepseek-v4-flash".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.14,
            cached_price: 0.014,
            output_price: 0.28,
            models: vec!["deepseek-v4-flash".to_string()],
            model_specs: vec![ModelSpec {
                name: "deepseek-v4-flash".to_string(),
                tier: ModelTier::Flagship,
                context_window: "1M".to_string(),
                max_output: "32K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
            }],
        },
    );

    config.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", upstream_addr),
            default_model: "gpt-4o-mini".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.15,
            cached_price: 0.075,
            output_price: 0.60,
            models: vec!["gpt-4o-mini".to_string()],
            model_specs: vec![ModelSpec {
                name: "gpt-4o-mini".to_string(),
                tier: ModelTier::Standard,
                context_window: "128K".to_string(),
                max_output: "16K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
            }],
        },
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek", pool_ds);
    state.register_pool("openai", pool_openai);

    let gateway_app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // 3. Test model echo policy: Requesting "deepseek-v4-flash[1m]:economy" MUST echo "deepseek-v4-flash[1m]:economy" in body
    let echo_resp = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "deepseek-v4-flash[1m]:economy",
            "messages": [{"role": "user", "content": "Hello"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(echo_resp.status(), 200);

    let routed_hdr = echo_resp.headers().get("x-ponyllm-routed-model").unwrap().to_str().unwrap();
    assert_eq!(routed_hdr, "deepseek-v4-flash");

    let body: serde_json::Value = echo_resp.json().await.unwrap();
    assert_eq!(body["model"], "deepseek-v4-flash[1m]:economy");

    // 4. Test auto default routing (resolves to Standard tier: gpt-4o-mini)
    let auto_resp = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "auto",
            "messages": [{"role": "user", "content": "Hello auto"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(auto_resp.status(), 200);
    assert_eq!(auto_resp.headers().get("x-ponyllm-routed-model").unwrap().to_str().unwrap(), "gpt-4o-mini");
    let auto_body: serde_json::Value = auto_resp.json().await.unwrap();
    assert_eq!(auto_body["model"], "auto");

    // 5. Test auto:flagship routing (resolves to Flagship tier: deepseek-v4-flash)
    let auto_flag_resp = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "auto:flagship",
            "messages": [{"role": "user", "content": "Hello flagship"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(auto_flag_resp.status(), 200);
    assert_eq!(auto_flag_resp.headers().get("x-ponyllm-routed-model").unwrap().to_str().unwrap(), "deepseek-v4-flash");

    // 6. Test auto[1m] Adaptive Tier Elevation: Standard has only 128K, so auto[1m] MUST elevate to Flagship 1M!
    let auto_1m_resp = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "auto[1m]",
            "messages": [{"role": "user", "content": "Hello auto 1m"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(auto_1m_resp.status(), 200);
    assert_eq!(auto_1m_resp.headers().get("x-ponyllm-routed-model").unwrap().to_str().unwrap(), "deepseek-v4-flash");
    let auto_1m_body: serde_json::Value = auto_1m_resp.json().await.unwrap();
    assert_eq!(auto_1m_body["model"], "auto[1m]");

    // 7. Test /v1/models listing: must contain auto, auto:standard, auto:flagship, and [1m] alias
    let models_resp = client
        .get(format!("http://{}/v1/models", gateway_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(models_resp.status(), 200);
    let models_json: serde_json::Value = models_resp.json().await.unwrap();
    let model_ids: Vec<&str> = models_json["data"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    
    assert!(model_ids.contains(&"auto"));
    assert!(model_ids.contains(&"auto:standard"));
    assert!(model_ids.contains(&"auto:flagship"));
    assert!(model_ids.contains(&"auto:economy"));
    assert!(model_ids.contains(&"auto:fastest"));
    assert!(model_ids.contains(&"deepseek-v4-flash"));
    assert!(model_ids.contains(&"deepseek-v4-flash[1m]"));

    // 8. Test single model GET /v1/models/:model_id
    let single_auto_resp = client
        .get(format!("http://{}/v1/models/auto:flagship", gateway_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(single_auto_resp.status(), 200);
    let single_auto_json: serde_json::Value = single_auto_resp.json().await.unwrap();
    assert_eq!(single_auto_json["id"], "auto:flagship");
}

#[tokio::test]
async fn test_anthropic_messages_routing_and_model_echo() {
    // Mock Anthropic upstream server
    let mock_anthropic = Router::new().route(
        "/v1/messages",
        post(|Json(req): Json<serde_json::Value>| async move {
            let m = req["model"].as_str().unwrap_or_default().to_string();
            axum::Json(json!({
                "id": "msg_mock_789",
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "text",
                    "text": "Hello Anthropic Echo"
                }],
                "model": m,
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {
                    "input_tokens": 20,
                    "output_tokens": 10
                }
            }))
        }),
    );

    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock_anthropic).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("anthropic", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("ant-k1", "sk-ant-key", 1, 10));

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "anthropic".to_string(),
        ProviderConfig {
            base_url: format!("http://{}/v1/messages", upstream_addr),
            default_model: "claude-3-7-sonnet".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 3.0,
            cached_price: 0.3,
            output_price: 15.0,
            models: vec!["claude-3-7-sonnet".to_string()],
            model_specs: vec![ModelSpec {
                name: "claude-3-7-sonnet".to_string(),
                tier: ModelTier::Flagship,
                context_window: "1M".to_string(),
                max_output: "64K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
            }],
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

    // Send /v1/messages request with "claude-3-7-sonnet[1m]:speed"
    let resp = client
        .post(format!("http://{}/v1/messages", gateway_addr))
        .json(&json!({
            "model": "claude-3-7-sonnet[1m]:speed",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "Hello claude"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("x-ponyllm-routed-model").unwrap().to_str().unwrap(), "claude-3-7-sonnet");
    assert_eq!(resp.headers().get("x-ponyllm-strategy").unwrap().to_str().unwrap(), "speed");
    assert_eq!(resp.headers().get("x-ponyllm-tier").unwrap().to_str().unwrap(), "F");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["model"], "claude-3-7-sonnet[1m]:speed");
}
