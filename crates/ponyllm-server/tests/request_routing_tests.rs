#![allow(clippy::field_reassign_with_default)]

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
                billing_mode: None,
                input_price: None,
                cached_price: None,
                output_price: None,
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
                billing_mode: None,
                input_price: None,
                cached_price: None,
                output_price: None,
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

#[test]
fn test_is_anthropic_upstream_heuristic_lock() {
    use ponyllm_server::{AppState, GatewayConfig, ProviderConfig};
    use ponyllm_server::routes::models::ParsedRequestModel;

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "ant-p".to_string(),
        ProviderConfig {
            base_url: "https://api.deepseek.com/anthropic".to_string(),
            default_model: "m-ant".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.1,
            cached_price: 0.01,
            output_price: 0.2,
            models: vec![],
            model_specs: vec![],
        },
    );
    config.providers.insert(
        "chat-p".to_string(),
        ProviderConfig {
            base_url: "https://api.deepseek.com".to_string(),
            default_model: "m-chat".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.1,
            cached_price: 0.01,
            output_price: 0.2,
            models: vec![],
            model_specs: vec![],
        },
    );
    let state = AppState::new(config);

    let ant = state
        .resolve_routed_targets(&ParsedRequestModel::parse("m-ant"), None)
        .unwrap();
    assert_eq!(ant.len(), 1);
    assert!(ant[0].is_anthropic_upstream);

    let chat = state
        .resolve_routed_targets(&ParsedRequestModel::parse("m-chat"), None)
        .unwrap();
    assert_eq!(chat.len(), 1);
    assert!(!chat[0].is_anthropic_upstream);
}

#[test]
fn test_exhausted_message_distinguishes_local_pool() {
    use ponyllm_server::extractors::format_exhausted_message;
    let local = format_exhausted_message(
        "Request failed after 0 retries: No available key in pool: foo",
        "req_1",
    );
    assert!(local.contains("Local key pool exhausted"));
    assert!(local.contains("req_1"));
    let upstream = format_exhausted_message("HTTP 500 from k1: boom", "req_2");
    assert!(upstream.contains("All candidate upstream providers exhausted"));
}

#[tokio::test]
async fn test_cross_provider_transparent_failover() {
    // 1. Setup healthy secondary upstream mock
    let healthy_upstream = Router::new().route(
        "/v1/chat/completions",
        post(|Json(_req): Json<serde_json::Value>| async move {
            axum::Json(json!({
                "id": "chatcmpl-backup-123",
                "object": "chat.completion",
                "created": 1710000000,
                "model": "deepseek-v4-flash",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hello from healthy backup provider!"
                    },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
            }))
        }),
    );
    let healthy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let healthy_addr = healthy_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(healthy_listener, healthy_upstream).await.unwrap();
    });

    // 2. Setup gateway with broken primary provider (bad url) and healthy backup provider
    let pool_broken = Arc::new(KeyPool::new("broken_provider", RoutingStrategy::RoundRobin));
    pool_broken.add_key(ApiKeyEntry::new("broken-k1", "sk-broken", 1, 10));

    let pool_backup = Arc::new(KeyPool::new("backup_provider", RoutingStrategy::RoundRobin));
    pool_backup.add_key(ApiKeyEntry::new("backup-k1", "sk-backup", 1, 10));

    let mut config = GatewayConfig::default();
    config.max_retries = 1;

    // Primary broken provider has slightly lower price to be preferred first
    config.providers.insert(
        "broken_provider".to_string(),
        ProviderConfig {
            base_url: "http://127.0.0.1:1".to_string(), // Dead port
            default_model: "deepseek-v4-flash".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.10,
            cached_price: 0.01,
            output_price: 0.20,
            models: vec!["deepseek-v4-flash".to_string()],
            model_specs: vec![ModelSpec {
                name: "deepseek-v4-flash".to_string(),
                tier: ModelTier::Flagship,
                context_window: "1M".to_string(),
                max_output: "32K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                billing_mode: None,
                input_price: None,
                cached_price: None,
                output_price: None,
            }],
        },
    );

    config.providers.insert(
        "backup_provider".to_string(),
        ProviderConfig {
            base_url: format!("http://{}", healthy_addr),
            default_model: "deepseek-v4-flash".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.20,
            cached_price: 0.02,
            output_price: 0.40,
            models: vec!["deepseek-v4-flash".to_string()],
            model_specs: vec![ModelSpec {
                name: "deepseek-v4-flash".to_string(),
                tier: ModelTier::Flagship,
                context_window: "1M".to_string(),
                max_output: "32K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                billing_mode: None,
                input_price: None,
                cached_price: None,
                output_price: None,
            }],
        },
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("broken_provider", pool_broken);
    state.register_pool("backup_provider", pool_backup);

    let gateway_app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // 3. Send request: broken provider fails, gateway MUST transparently failover to backup provider!
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "messages": [{"role": "user", "content": "Hello failover"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("x-ponyllm-provider").unwrap().to_str().unwrap(), "backup_provider");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "Hello from healthy backup provider!");
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
                billing_mode: None,
                input_price: None,
                cached_price: None,
                output_price: None,
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

    // Also send /v1/messages with system in messages[1] (as sent by Claude Code)
    let resp_with_sys = client
        .post(format!("http://{}/v1/messages", gateway_addr))
        .json(&json!({
            "model": "claude-3-7-sonnet[1m]:speed",
            "max_tokens": 1024,
            "messages": [
                {"role": "user", "content": "Hello claude"},
                {"role": "system", "content": "System instruction in messages array by Claude Code"},
                {"role": "assistant", "content": "Understood"}
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp_with_sys.status(), 200);
}

#[tokio::test]
async fn test_gateway_configuration_hot_reload() {
    let mock_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_b_addr = mock_b.local_addr().unwrap();

    // Mock Upstream B
    tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|axum::Json(req): axum::Json<serde_json::Value>| async move {
                axum::Json(json!({
                    "id": "chatcmpl-b",
                    "object": "chat.completion",
                    "created": 123456789,
                    "model": req["model"],
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "Hello from Provider B"},
                        "finish_reason": "stop"
                    }]
                }))
            }),
        );
        axum::serve(mock_b, app).await.unwrap();
    });

    // 1. Initial configuration: prov_a only
    let mut gw_config = GatewayConfig::default();
    gw_config.bind_addr = "127.0.0.1:0".to_string();
    gw_config.api_key = String::new();
    gw_config.providers.insert(
        "prov_a".to_string(),
        ProviderConfig {
            base_url: "http://127.0.0.1:12345/v1".to_string(),
            default_model: "model-a".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 1.0,
            cached_price: 0.5,
            output_price: 2.0,
            models: vec!["model-a".to_string()],
            model_specs: vec![],
        },
    );

    let state = Arc::new(AppState::new(gw_config.clone()));
    let pool_a = Arc::new(KeyPool::new("prov_a", RoutingStrategy::RoundRobin));
    pool_a.add_key(ApiKeyEntry::new("key-a", "sk-a", 1, 1));
    state.register_pool("prov_a", pool_a);

    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    let app = create_app(state.clone());
    tokio::spawn(async move {
        axum::serve(gateway_listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // 2. Query /v1/models before reload: only model-a exists
    let models_1: serde_json::Value = client
        .get(format!("http://{}/v1/models", gateway_addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids_1: Vec<&str> = models_1["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(ids_1.contains(&"model-a"));
    assert!(!ids_1.contains(&"model-b"));

    // 3. Perform Hot Reload: remove prov_a, add prov_b
    let mut new_config = GatewayConfig::default();
    new_config.bind_addr = gw_config.bind_addr.clone();
    new_config.providers.insert(
        "prov_b".to_string(),
        ProviderConfig {
            base_url: format!("http://{}/v1", mock_b_addr),
            default_model: "model-b".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.1,
            cached_price: 0.05,
            output_price: 0.2,
            models: vec!["model-b".to_string()],
            model_specs: vec![],
        },
    );

    let mut new_pools = std::collections::HashMap::new();
    let pool_b = Arc::new(KeyPool::new("prov_b", RoutingStrategy::RoundRobin));
    pool_b.add_key(ApiKeyEntry::new("key-b", "sk-b", 1, 1));
    new_pools.insert("prov_b".to_string(), pool_b);

    state.reload_config_with_pools(new_config, new_pools);

    // 4. Query /v1/models after reload: model-a is gone, model-b is live!
    let models_2: serde_json::Value = client
        .get(format!("http://{}/v1/models", gateway_addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids_2: Vec<&str> = models_2["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(!ids_2.contains(&"model-a"), "Old model-a should be removed");
    assert!(ids_2.contains(&"model-b"), "New model-b should be exposed");

    // 5. Query chat completions with model-b: should succeed seamlessly
    let chat_resp = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "model-b",
            "messages": [{"role": "user", "content": "Hello b"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(chat_resp.status(), 200);
    assert_eq!(
        chat_resp.headers().get("x-ponyllm-provider").unwrap().to_str().unwrap(),
        "prov_b"
    );
    let chat_json: serde_json::Value = chat_resp.json().await.unwrap();
    assert_eq!(chat_json["choices"][0]["message"]["content"], "Hello from Provider B");
}

#[tokio::test]
async fn test_large_payload_handling_with_1m_context_support() {
    // 1. Mock upstream that echoes received payload length
    let mock_upstream = Router::new()
        .route(
            "/v1/chat/completions",
            post(|Json(req): Json<serde_json::Value>| async move {
                let messages = req["messages"].as_array().unwrap();
                let content_len = messages[0]["content"].as_str().unwrap().len();
                axum::Json(json!({
                    "id": "chatcmpl-large-context",
                    "object": "chat.completion",
                    "created": 1710000000,
                    "model": "deepseek-v4-flash",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": format!("Received {} bytes", content_len)
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 100000,
                        "completion_tokens": 10,
                        "total_tokens": 100010
                    }
                }))
            }),
        )
        .layer(axum::extract::DefaultBodyLimit::max(128 * 1024 * 1024));

    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock_upstream).await.unwrap();
    });

    // 2. Gateway setup with default 128MB body limit and deepseek provider
    let pool = Arc::new(KeyPool::new("deepseek", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("ds-1", "sk-ds-key", 1, 10));

    let mut config = GatewayConfig::default();
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
                billing_mode: None,
                input_price: None,
                cached_price: None,
                output_price: None,
            }],
        },
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek", pool);

    let app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, app).await.unwrap();
    });

    // 3. Construct a 3MB payload (> 2MB default Axum limit)
    let large_text = "A".repeat(3 * 1024 * 1024); // 3 MiB string
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "messages": [{"role": "user", "content": large_text}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Large payload >2MB must succeed through gateway");
    let resp_json: serde_json::Value = resp.json().await.unwrap();
    assert!(resp_json["choices"][0]["message"]["content"].as_str().unwrap().contains("Received 3145728 bytes"));
}

#[tokio::test]
async fn test_custom_request_body_limit_rejection_with_helpful_error() {
    let pool = Arc::new(KeyPool::new("test-p", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));

    let mut config = GatewayConfig::default();
    config.request_body_limit = 16 * 1024; // 16 KB small limit
    config.providers.insert(
        "test-p".to_string(),
        ProviderConfig {
            base_url: "http://127.0.0.1:9".to_string(),
            default_model: "test-model".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.1,
            cached_price: 0.01,
            output_price: 0.2,
            models: vec!["test-model".to_string()],
            model_specs: vec![],
        },
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("test-p", pool);

    let app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, app).await.unwrap();
    });

    // Send a 32 KB payload (> 16 KB limit)
    let text_32k = "B".repeat(32 * 1024);
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gateway_addr))
        .json(&json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": text_32k}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    let err_json: serde_json::Value = resp.json().await.unwrap();
    let err_msg = err_json["error"]["message"].as_str().unwrap();
    assert!(err_msg.contains("Request body length limit exceeded") || err_msg.contains("length limit exceeded"));
}


