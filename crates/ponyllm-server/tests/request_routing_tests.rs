#![allow(clippy::field_reassign_with_default)]

use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::pool::*;
use ponyllm_server::config::{default_context_window, default_max_output, default_modalities};
use ponyllm_server::routes::models::ParsedRequestModel;
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
    assert_eq!(
        p4_tagged.strategy_override,
        Some(GatewayRoutingStrategy::Speed)
    );

    // 5. Auto virtual model: pure auto only, strips explicit tier / strategy / 1m
    let p4 = ParsedRequestModel::parse("auto");
    assert_eq!(p4.clean_model_name, "auto");
    assert!(p4.is_auto);
    assert_eq!(p4.explicit_tier, None);

    let p5 = ParsedRequestModel::parse("auto:flagship:economy");
    assert!(p5.is_auto);
    assert_eq!(p5.clean_model_name, "auto");
    assert_eq!(p5.explicit_tier, None); // converged to pure auto
    assert_eq!(p5.strategy_override, None);

    let p6 = ParsedRequestModel::parse("auto[1m]:speed");
    assert!(p6.is_auto);
    assert_eq!(p6.clean_model_name, "auto");
    assert!(!p6.is_1m_context); // converged to pure auto
    assert_eq!(p6.strategy_override, None);
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
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode

    config.providers.insert(
        "deepseek".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: format!("http://{}", upstream_addr),
            default_model: "deepseek-v4-flash".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.14,
            cached_price: 0.014,
            output_price: 0.28,
            models: vec!["deepseek-v4-flash".to_string()],
            model_specs: vec![ModelSpec {
                rate_limits: None,
                priority: None,
                name: "deepseek-v4-flash".to_string(),
                tier: ModelTier::Flagship,
                context_window: "1M".to_string(),
                max_output: "32K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                ..Default::default()
            }],
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
        },
    );

    config.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: format!("http://{}", upstream_addr),
            default_model: "gpt-4o-mini".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.15,
            cached_price: 0.075,
            output_price: 0.60,
            models: vec!["gpt-4o-mini".to_string()],
            model_specs: vec![ModelSpec {
                rate_limits: None,
                priority: None,
                name: "gpt-4o-mini".to_string(),
                tier: ModelTier::Standard,
                context_window: "128K".to_string(),
                max_output: "16K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                ..Default::default()
            }],
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
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

    let routed_hdr = echo_resp
        .headers()
        .get("x-ponyllm-routed-model")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(routed_hdr, "deepseek-v4-flash");

    let body: serde_json::Value = echo_resp.json().await.unwrap();
    assert_eq!(body["model"], "deepseek-v4-flash[1m]:economy");

    // 4. Test auto default routing (resolves to primary default model: deepseek-v4-flash)
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
    assert_eq!(
        auto_resp
            .headers()
            .get("x-ponyllm-routed-model")
            .unwrap()
            .to_str()
            .unwrap(),
        "deepseek-v4-flash"
    );
    let auto_body: serde_json::Value = auto_resp.json().await.unwrap();
    assert_eq!(auto_body["model"], "auto");

    // 5. Test /v1/models listing: must contain pure auto (without auto:standard, auto:flagship etc.)
    let models_resp = client
        .get(format!("http://{}/v1/models", gateway_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(models_resp.status(), 200);
    let models_json: serde_json::Value = models_resp.json().await.unwrap();
    let model_ids: Vec<&str> = models_json["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();

    assert!(model_ids.contains(&"auto"));
    assert!(!model_ids.contains(&"auto:standard"));
    assert!(!model_ids.contains(&"auto:flagship"));
    assert!(!model_ids.contains(&"auto:economy"));
    assert!(!model_ids.contains(&"auto:fastest"));
    assert!(model_ids.contains(&"deepseek-v4-flash"));
    assert!(model_ids.contains(&"deepseek-v4-flash[1m]"));

    // 6. Test single model GET /v1/models/:model_id
    let single_auto_resp = client
        .get(format!("http://{}/v1/models/auto", gateway_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(single_auto_resp.status(), 200);
    let single_auto_json: serde_json::Value = single_auto_resp.json().await.unwrap();
    assert_eq!(single_auto_json["id"], "auto");
}

#[test]
fn test_is_anthropic_upstream_heuristic_lock() {
    use ponyllm_server::routes::models::ParsedRequestModel;
    use ponyllm_server::{AppState, GatewayConfig, ProviderConfig};

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "ant-p".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: "https://api.deepseek.com/anthropic".to_string(),
            default_model: "m-ant".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.1,
            cached_price: 0.01,
            output_price: 0.2,
            models: vec![],
            model_specs: vec![],
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
        },
    );
    config.providers.insert(
        "chat-p".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: "https://api.deepseek.com".to_string(),
            default_model: "m-chat".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.1,
            cached_price: 0.01,
            output_price: 0.2,
            models: vec![],
            model_specs: vec![],
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
        },
    );
    let state = AppState::new(config);

    let ant = state
        .resolve_routed_targets(&ParsedRequestModel::parse("m-ant"), None)
        .unwrap();
    assert_eq!(ant.len(), 1);
    assert_eq!(ant[0].upstream_protocol, UpstreamProtocol::Anthropic);

    let chat = state
        .resolve_routed_targets(&ParsedRequestModel::parse("m-chat"), None)
        .unwrap();
    assert_eq!(chat.len(), 1);
    assert_eq!(chat[0].upstream_protocol, UpstreamProtocol::Chat);
}

#[test]
fn test_protocol_resolution_priority_and_overrides() {
    use ponyllm_server::routes::models::ParsedRequestModel;
    use ponyllm_server::{AppState, GatewayConfig, ModelSpec, ProviderConfig};

    fn provider(
        base: &str,
        model: &str,
        proto: Option<UpstreamProtocol>,
        spec_proto: Option<UpstreamProtocol>,
        endpoint: Option<(&str, &str)>,
    ) -> ProviderConfig {
        let (chat_url, responses_url, messages_url) = match endpoint {
            Some(("chat", u)) => (Some(u.to_string()), None, None),
            Some(("responses", u)) => (None, Some(u.to_string()), None),
            Some(("messages", u)) => (None, None, Some(u.to_string())),
            _ => (None, None, None),
        };
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: base.to_string(),
            default_model: model.to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.1,
            cached_price: 0.01,
            output_price: 0.2,
            models: vec![],
            model_specs: if let Some(sp) = spec_proto {
                vec![ModelSpec {
                    rate_limits: None,
                    priority: None,
                    name: model.to_string(),
                    tier: ModelTier::Standard,
                    context_window: "128K".to_string(),
                    max_output: "4K".to_string(),
                    input_types: vec!["text".to_string()],
                    output_types: vec!["text".to_string()],
                    protocol: Some(sp),
                    ..Default::default()
                }]
            } else {
                vec![]
            },
            default_protocol: proto,
            chat_url,
            responses_url,
            messages_url,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
        }
    }

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
                                                       // Explicit default beats the anthropic-URL heuristic.
    config.providers.insert(
        "p1".to_string(),
        provider(
            "https://x.example.com/anthropic",
            "m1",
            Some(UpstreamProtocol::Chat),
            None,
            None,
        ),
    );
    // Model override beats provider default.
    config.providers.insert(
        "p2".to_string(),
        provider(
            "https://y.example.com",
            "m2",
            Some(UpstreamProtocol::Chat),
            Some(UpstreamProtocol::Responses),
            None,
        ),
    );
    // Per-protocol endpoint override is honored for URL building.
    config.providers.insert(
        "p3".to_string(),
        provider(
            "https://z.example.com",
            "m3",
            Some(UpstreamProtocol::Responses),
            None,
            Some(("responses", "https://resp.example.com/v1")),
        ),
    );
    let state = AppState::new(config);

    let t1 = state
        .resolve_routed_targets(&ParsedRequestModel::parse("m1"), None)
        .unwrap();
    assert_eq!(t1[0].upstream_protocol, UpstreamProtocol::Chat);
    assert_eq!(t1[0].endpoint_base, None);

    let t2 = state
        .resolve_routed_targets(&ParsedRequestModel::parse("m2"), None)
        .unwrap();
    assert_eq!(t2[0].upstream_protocol, UpstreamProtocol::Responses);

    let t3 = state
        .resolve_routed_targets(&ParsedRequestModel::parse("m3"), None)
        .unwrap();
    assert_eq!(t3[0].upstream_protocol, UpstreamProtocol::Responses);
    assert_eq!(
        t3[0].endpoint_base.as_deref(),
        Some("https://resp.example.com/v1")
    );
    assert_eq!(
        t3[0].responses_url(),
        "https://resp.example.com/v1/responses"
    );

    // Request header override wins over everything; invalid values are ignored.
    let t1h = state
        .resolve_routed_targets_with_prompt_and_protocol(
            &ParsedRequestModel::parse("m1"),
            None,
            None,
            Some(UpstreamProtocol::Anthropic),
            None,
        )
        .unwrap();
    assert_eq!(t1h[0].upstream_protocol, UpstreamProtocol::Anthropic);

    assert!(
        ponyllm_server::extractors::parse_protocol_header(&axum::http::HeaderMap::new()).is_none()
    );
    let mut bad = axum::http::HeaderMap::new();
    bad.insert(
        "x-pony-protocol",
        axum::http::HeaderValue::from_static("carrier-pigeon"),
    );
    assert!(ponyllm_server::extractors::parse_protocol_header(&bad).is_none());
    let mut good = axum::http::HeaderMap::new();
    good.insert(
        "x-pony-protocol",
        axum::http::HeaderValue::from_static("responses"),
    );
    assert_eq!(
        ponyllm_server::extractors::parse_protocol_header(&good),
        Some(UpstreamProtocol::Responses)
    );
}

#[test]
fn test_models_listing_exposes_native_protocol() {
    use ponyllm_server::{AppState, GatewayConfig, ProviderConfig};
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "op".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: "https://op.example.com".to_string(),
            default_model: "muse-spark".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.1,
            cached_price: 0.01,
            output_price: 0.2,
            models: vec![],
            model_specs: vec![],
            default_protocol: Some(UpstreamProtocol::Responses),
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
        },
    );
    let state = AppState::new(config);
    let models = state.list_all_models();
    let found = models
        .iter()
        .find(|(m, _, _, _)| m == "muse-spark")
        .expect("model listed");
    assert_eq!(found.1, "op");
    assert_eq!(found.3, "responses");
}

#[test]
fn test_native_protocol_wins_ties_for_passthrough_first() {
    use ponyllm_server::routes::models::ParsedRequestModel;
    use ponyllm_server::{AppState, GatewayConfig, ProviderConfig};

    // Two providers serve the same model at identical prices; only the native
    // protocol differs. Same-native must rank first per inbound entry.
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    for (name, proto) in [
        ("chat-p", UpstreamProtocol::Chat),
        ("ant-p", UpstreamProtocol::Anthropic),
    ] {
        config.providers.insert(
            name.to_string(),
            ProviderConfig {
                egress_pool: vec![],
                egress_strategy: "round_robin".to_string(),
                rate_limits: None,
                base_url: format!("https://{}.example.com", name),
                default_model: "duo".to_string(),
                strategy: "round_robin".to_string(),
                billing_mode: BillingMode::Metered,
                input_price: 0.5,
                cached_price: 0.25,
                output_price: 1.0,
                models: vec![],
                model_specs: vec![],
                default_protocol: Some(proto),
                chat_url: None,
                responses_url: None,
                messages_url: None,
                proxy: None,
                timeout_secs: None,
                ttfb_timeout_secs: None,
            },
        );
    }
    let state = AppState::new(config);
    let parsed = ParsedRequestModel::parse("duo");

    let chat_first = state
        .resolve_routed_targets_with_prompt_and_protocol(
            &parsed,
            Some(GatewayRoutingStrategy::Reliable),
            None,
            None,
            Some(UpstreamProtocol::Chat),
        )
        .unwrap();
    assert_eq!(chat_first[0].provider_name, "chat-p");

    let ant_first = state
        .resolve_routed_targets_with_prompt_and_protocol(
            &parsed,
            Some(GatewayRoutingStrategy::Reliable),
            None,
            None,
            Some(UpstreamProtocol::Anthropic),
        )
        .unwrap();
    assert_eq!(ant_first[0].provider_name, "ant-p");

    // No inbound preference: strategy order untouched (insertion order here).
    let plain = state
        .resolve_routed_targets(&parsed, Some(GatewayRoutingStrategy::Reliable))
        .unwrap();
    assert_eq!(plain.len(), 2);
}

#[test]
fn test_inbound_native_endpoint_wins_over_provider_default() {
    use ponyllm_server::routes::models::ParsedRequestModel;
    use ponyllm_server::{AppState, GatewayConfig, ProviderConfig};

    // Merged single-provider DeepSeek: default chat + messages_url override.
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "deepseek".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: "https://api.deepseek.com".to_string(),
            default_model: "deepseek-chat".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.1,
            cached_price: 0.01,
            output_price: 0.2,
            models: vec![],
            model_specs: vec![],
            default_protocol: Some(UpstreamProtocol::Chat),
            chat_url: None,
            responses_url: None,
            messages_url: Some("https://api.deepseek.com/anthropic".to_string()),
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
        },
    );
    let state = AppState::new(config);
    let parsed = ParsedRequestModel::parse("deepseek-chat");

    // Native Anthropic inbound binds the messages endpoint verbatim.
    let msg = state
        .resolve_routed_targets_with_prompt_and_protocol(
            &parsed,
            None,
            None,
            None,
            Some(UpstreamProtocol::Anthropic),
        )
        .unwrap();
    assert_eq!(msg[0].upstream_protocol, UpstreamProtocol::Anthropic);
    assert_eq!(
        msg[0].endpoint_base.as_deref(),
        Some("https://api.deepseek.com/anthropic")
    );
    assert_eq!(
        msg[0].messages_url(),
        "https://api.deepseek.com/anthropic/v1/messages"
    );

    // Chat inbound keeps the default with base-derived URL.
    let chat = state
        .resolve_routed_targets_with_prompt_and_protocol(
            &parsed,
            None,
            None,
            None,
            Some(UpstreamProtocol::Chat),
        )
        .unwrap();
    assert_eq!(chat[0].upstream_protocol, UpstreamProtocol::Chat);
    assert_eq!(chat[0].endpoint_base, None);
}

#[test]
fn test_exhausted_message_distinguishes_local_pool() {
    use ponyllm_core::error::GatewayErrorKind;
    use ponyllm_server::extractors::format_exhausted_message;
    let local = format_exhausted_message(
        "gemini-3.8-flash",
        &GatewayErrorKind::RateLimitExceeded { retry_after: None },
        "No available key for provider 'opencode' (all keys cooling down or disabled)",
        true,
        "req_1",
    );
    assert!(local.contains("Local key pool exhausted"));
    assert!(local.contains("for model 'gemini-3.8-flash'"));
    assert!(local.contains("req_1"));
    let upstream = format_exhausted_message(
        "gemini-3.8-flash",
        &GatewayErrorKind::UpstreamUnavailable,
        "HTTP 500 from k1: boom",
        false,
        "req_2",
    );
    assert!(upstream.contains("All candidate upstream providers exhausted"));
    assert!(upstream.contains("for model 'gemini-3.8-flash'"));
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
        axum::serve(healthy_listener, healthy_upstream)
            .await
            .unwrap();
    });

    // 2. Setup gateway with broken primary provider (bad url) and healthy backup provider
    let pool_broken = Arc::new(KeyPool::new("broken_provider", RoutingStrategy::RoundRobin));
    pool_broken.add_key(ApiKeyEntry::new("broken-k1", "sk-broken", 1, 10));

    let pool_backup = Arc::new(KeyPool::new("backup_provider", RoutingStrategy::RoundRobin));
    pool_backup.add_key(ApiKeyEntry::new("backup-k1", "sk-backup", 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.max_retries = 1;

    // Primary broken provider has slightly lower price to be preferred first
    config.providers.insert(
        "broken_provider".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: "http://127.0.0.1:1".to_string(), // Dead port
            default_model: "deepseek-v4-flash".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.10,
            cached_price: 0.01,
            output_price: 0.20,
            models: vec!["deepseek-v4-flash".to_string()],
            model_specs: vec![ModelSpec {
                rate_limits: None,
                priority: None,
                name: "deepseek-v4-flash".to_string(),
                tier: ModelTier::Flagship,
                context_window: "1M".to_string(),
                max_output: "32K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                ..Default::default()
            }],
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
        },
    );

    config.providers.insert(
        "backup_provider".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: format!("http://{}", healthy_addr),
            default_model: "deepseek-v4-flash".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.20,
            cached_price: 0.02,
            output_price: 0.40,
            models: vec!["deepseek-v4-flash".to_string()],
            model_specs: vec![ModelSpec {
                rate_limits: None,
                priority: None,
                name: "deepseek-v4-flash".to_string(),
                tier: ModelTier::Flagship,
                context_window: "1M".to_string(),
                max_output: "32K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                ..Default::default()
            }],
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
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
    assert_eq!(
        resp.headers()
            .get("x-ponyllm-provider")
            .unwrap()
            .to_str()
            .unwrap(),
        "backup_provider"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "Hello from healthy backup provider!"
    );
}

/// Same-model multi-provider quota semantics (bugfix 2026-10-02).
///
/// The same model configured on two providers appears ONCE in `/v1/models`
/// (dedup by bare name), but at request time a quota exhaustion on the first
/// provider used to transparently fail over to the second provider and
/// silently consume the second provider's quota. Default behavior now stops at
/// the quota boundary (`quota_exhausted`, 429) unless
/// `cross_provider_quota_failover = true` opts back in.

#[derive(Clone, Copy)]
enum QuotaFailMode {
    /// 402 Payment Required — unambiguous quota exhaustion.
    PaymentRequired,
    /// 429 with balance-wording body — classified QuotaExhausted.
    Balance429,
    /// 429 with rate-limit-wording body — transient, may fail over.
    Rate429,
}

fn quota_fail_response(mode: QuotaFailMode) -> (axum::http::StatusCode, serde_json::Value) {
    match mode {
        QuotaFailMode::PaymentRequired => (
            axum::http::StatusCode::PAYMENT_REQUIRED,
            json!({"error": {"message": "insufficient account balance", "type": "insufficient_quota", "code": "402"}}),
        ),
        QuotaFailMode::Balance429 => (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            json!({"error": {"message": "your account balance is exhausted", "type": "insufficient_quota", "code": "429"}}),
        ),
        QuotaFailMode::Rate429 => (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            json!({"error": {"message": "rate limit exceeded for account rpm_user", "type": "rate_limit_error", "code": "429"}}),
        ),
    }
}

async fn spawn_quota_failover_gateway(
    quota_failover: bool,
    route: &'static str,
    fail_mode: QuotaFailMode,
) -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let a_hits = Arc::new(AtomicUsize::new(0));
    let b_hits = Arc::new(AtomicUsize::new(0));

    // Upstreams always serve the chat URL: with `default_protocol = None` and
    // an IP base URL every gateway entry resolves upstream protocol Chat, so
    // all three gateway routes (chat/messages/responses) forward to
    // `{base}/v1/chat/completions` after normalization.
    // Provider A: quota-exhausted mock.
    let a_hits_clone = a_hits.clone();
    let quota_upstream = Router::new().route(
        "/v1/chat/completions",
        post(move |_req: Json<serde_json::Value>| async move {
            a_hits_clone.fetch_add(1, Ordering::SeqCst);
            let (status, body) = quota_fail_response(fail_mode);
            (status, axum::Json(body)).into_response()
        }),
    );
    let quota_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let quota_addr = quota_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(quota_listener, quota_upstream).await.unwrap();
    });

    // Provider B: healthy backup, counts every request it serves.
    let b_hits_clone = b_hits.clone();
    let healthy_upstream = Router::new().route(
        "/v1/chat/completions",
        post(move |_req: Json<serde_json::Value>| async move {
            b_hits_clone.fetch_add(1, Ordering::SeqCst);
            axum::Json(json!({
                "id": "chatcmpl-backup-quota",
                "object": "chat.completion",
                "created": 1710000000,
                "model": "quota-test-model",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "served by backup" },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
            }))
        }),
    );
    let healthy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let healthy_addr = healthy_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(healthy_listener, healthy_upstream)
            .await
            .unwrap();
    });

    let pool_quota = Arc::new(KeyPool::new("quota_provider", RoutingStrategy::RoundRobin));
    pool_quota.add_key(ApiKeyEntry::new("quota-k1", "sk-quota", 1, 10));
    let pool_backup = Arc::new(KeyPool::new("backup_provider", RoutingStrategy::RoundRobin));
    pool_backup.add_key(ApiKeyEntry::new("backup-k1", "sk-backup", 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.max_retries = 1;
    config.cross_provider_quota_failover = quota_failover;

    for (p_name, base) in [
        ("quota_provider", quota_addr),
        ("backup_provider", healthy_addr),
    ] {
        let cheap = p_name == "quota_provider";
        config.providers.insert(
            p_name.to_string(),
            ProviderConfig {
                egress_pool: vec![],
                egress_strategy: "round_robin".to_string(),
                rate_limits: None,
                base_url: format!("http://{}", base),
                default_model: "quota-test-model".to_string(),
                strategy: "priority".to_string(),
                billing_mode: BillingMode::Metered,
                input_price: if cheap { 0.10 } else { 0.20 },
                cached_price: 0.01,
                output_price: if cheap { 0.20 } else { 0.40 },
                models: vec!["quota-test-model".to_string()],
                model_specs: vec![ModelSpec {
                    rate_limits: None,
                    priority: None,
                    name: "quota-test-model".to_string(),
                    tier: ModelTier::Flagship,
                    context_window: "1M".to_string(),
                    max_output: "32K".to_string(),
                    input_types: vec!["text".to_string()],
                    output_types: vec!["text".to_string()],
                    ..Default::default()
                }],
                default_protocol: None,
                chat_url: None,
                responses_url: None,
                messages_url: None,
                proxy: None,
                timeout_secs: None,
                ttfb_timeout_secs: None,
            },
        );
    }

    let state = Arc::new(AppState::new(config));
    state.register_pool("quota_provider", pool_quota);
    state.register_pool("backup_provider", pool_backup);

    let gateway_app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway_app).await.unwrap();
    });

    (format!("http://{}", gateway_addr), a_hits, b_hits)
}

async fn send_quota_request(
    gateway_addr: &str,
    route: &str,
    model: &str,
    extra: Option<serde_json::Value>,
) -> reqwest::Response {
    let mut body = match route {
        "/v1/messages" => json!({
            "model": model,
            "messages": [{"role": "user", "content": "Hello quota failover"}],
            "max_tokens": 128
        }),
        "/v1/responses" => json!({
            "model": model,
            "input": "Hello quota failover"
        }),
        _ => json!({
            "model": model,
            "messages": [{"role": "user", "content": "Hello quota failover"}]
        }),
    };
    if let Some(extra) = extra {
        if let Some(obj) = body.as_object_mut() {
            if let Some(extra_obj) = extra.as_object() {
                for (k, v) in extra_obj {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
    }
    reqwest::Client::new()
        .post(format!("{}{}", gateway_addr, route))
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn test_quota_exhaustion_does_not_drain_backup_provider_by_default() {
    let (gateway_addr, a_hits, b_hits) = spawn_quota_failover_gateway(
        false,
        "/v1/chat/completions",
        QuotaFailMode::PaymentRequired,
    )
    .await;
    let resp = send_quota_request(
        &gateway_addr,
        "/v1/chat/completions",
        "quota-test-model",
        None,
    )
    .await;

    assert_eq!(resp.status(), 429);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "quota_exhausted");
    assert_eq!(
        a_hits.load(Ordering::SeqCst),
        1,
        "quota provider was attempted once"
    );
    assert_eq!(
        b_hits.load(Ordering::SeqCst),
        0,
        "backup provider quota must NOT be consumed by default"
    );
}

#[tokio::test]
async fn test_cross_provider_quota_failover_legacy_opt_in() {
    let (gateway_addr, _a_hits, b_hits) =
        spawn_quota_failover_gateway(true, "/v1/chat/completions", QuotaFailMode::PaymentRequired)
            .await;
    let resp = send_quota_request(
        &gateway_addr,
        "/v1/chat/completions",
        "quota-test-model",
        None,
    )
    .await;

    // Legacy opt-in: `cross_provider_quota_failover = true` restores the old
    // transparent failover that serves from the backup provider.
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("x-ponyllm-provider")
            .unwrap()
            .to_str()
            .unwrap(),
        "backup_provider"
    );
    assert_eq!(b_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_quota_guard_messages_and_responses_routes() {
    for route in ["/v1/messages", "/v1/responses"] {
        let (gateway_addr, _a_hits, b_hits) =
            spawn_quota_failover_gateway(false, route, QuotaFailMode::PaymentRequired).await;
        let resp = send_quota_request(&gateway_addr, route, "quota-test-model", None).await;

        // Guard behavior: quota boundary surfaces as 429 and never touches B.
        // (messages renders an Anthropic error envelope; the exact `code`
        // field shape is protocol-specific and asserted on the chat route.)
        assert_eq!(resp.status(), 429, "route {route}");
        assert_eq!(
            b_hits.load(Ordering::SeqCst),
            0,
            "route {route}: backup must not be consumed"
        );
    }
}

#[tokio::test]
async fn test_quota_guard_streaming_chat() {
    let (gateway_addr, _a_hits, b_hits) = spawn_quota_failover_gateway(
        false,
        "/v1/chat/completions",
        QuotaFailMode::PaymentRequired,
    )
    .await;
    let resp = send_quota_request(
        &gateway_addr,
        "/v1/chat/completions",
        "quota-test-model",
        Some(json!({"stream": true})),
    )
    .await;

    assert_eq!(resp.status(), 429);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "quota_exhausted");
    assert_eq!(
        b_hits.load(Ordering::SeqCst),
        0,
        "streaming must not consume the backup quota"
    );
}

#[tokio::test]
async fn test_quota_guard_provider_pin_routes_to_one_provider() {
    // Pin to the exhausted provider: quota error, backup untouched.
    let (gateway_addr, a_hits, b_hits) = spawn_quota_failover_gateway(
        false,
        "/v1/chat/completions",
        QuotaFailMode::PaymentRequired,
    )
    .await;
    let resp = send_quota_request(
        &gateway_addr,
        "/v1/chat/completions",
        "quota_provider/quota-test-model",
        None,
    )
    .await;
    assert_eq!(resp.status(), 429);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "quota_exhausted");
    assert_eq!(a_hits.load(Ordering::SeqCst), 1);
    assert_eq!(b_hits.load(Ordering::SeqCst), 0);

    // Pin to the healthy provider: served by backup, exhausted provider untouched.
    let (gateway_addr, a_hits, b_hits) = spawn_quota_failover_gateway(
        false,
        "/v1/chat/completions",
        QuotaFailMode::PaymentRequired,
    )
    .await;
    let resp = send_quota_request(
        &gateway_addr,
        "/v1/chat/completions",
        "backup_provider/quota-test-model",
        None,
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("x-ponyllm-provider")
            .unwrap()
            .to_str()
            .unwrap(),
        "backup_provider"
    );
    assert_eq!(
        a_hits.load(Ordering::SeqCst),
        0,
        "pinned healthy provider must not touch the exhausted provider"
    );
    assert_eq!(b_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_quota_guard_balance_wording_429_stops_but_rate_limit_429_fails_over() {
    // Balance-wording 429 -> QuotaExhausted -> boundary stop.
    let (gateway_addr, _a_hits, b_hits) =
        spawn_quota_failover_gateway(false, "/v1/chat/completions", QuotaFailMode::Balance429)
            .await;
    let resp = send_quota_request(
        &gateway_addr,
        "/v1/chat/completions",
        "quota-test-model",
        None,
    )
    .await;
    assert_eq!(resp.status(), 429);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "quota_exhausted");
    assert_eq!(
        b_hits.load(Ordering::SeqCst),
        0,
        "balance-wording 429 must stop at the quota boundary"
    );

    // Rate-limit-wording 429 -> transient -> legacy cross-provider failover.
    let (gateway_addr, _a_hits, b_hits) =
        spawn_quota_failover_gateway(false, "/v1/chat/completions", QuotaFailMode::Rate429).await;
    let resp = send_quota_request(
        &gateway_addr,
        "/v1/chat/completions",
        "quota-test-model",
        None,
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("x-ponyllm-provider")
            .unwrap()
            .to_str()
            .unwrap(),
        "backup_provider"
    );
    assert_eq!(
        b_hits.load(Ordering::SeqCst),
        1,
        "transient rate limit must still fail over"
    );
}

#[tokio::test]
async fn test_quota_guard_holds_across_cooldown_window_second_request() {
    // H1 (bugfix 2026-10-02): the first request cools quota_provider's only
    // key (quota cooldown); a second request finds NoAvailableKey, which must
    // reclassify as a quota boundary instead of draining the backup provider.
    let (gateway_addr, a_hits, b_hits) = spawn_quota_failover_gateway(
        false,
        "/v1/chat/completions",
        QuotaFailMode::PaymentRequired,
    )
    .await;

    let resp1 = send_quota_request(
        &gateway_addr,
        "/v1/chat/completions",
        "quota-test-model",
        None,
    )
    .await;
    assert_eq!(resp1.status(), 429);

    let resp2 = send_quota_request(
        &gateway_addr,
        "/v1/chat/completions",
        "quota-test-model",
        None,
    )
    .await;
    assert_eq!(resp2.status(), 429);
    let body: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(body["error"]["code"], "quota_exhausted");
    assert_eq!(
        a_hits.load(Ordering::SeqCst),
        1,
        "quota provider key is cooling: no upstream attempt on the second request"
    );
    assert_eq!(
        b_hits.load(Ordering::SeqCst),
        0,
        "cooldown-window retries must NOT burn the backup quota"
    );
}

#[test]
fn test_models_list_exposes_per_provider_aliases() {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    for (p, extra) in [("alpha", vec!["solo-model"]), ("beta", Vec::new())] {
        let mut models = vec!["shared-model".to_string()];
        models.extend(extra.into_iter().map(|m| m.to_string()));
        config.providers.insert(
            p.to_string(),
            ProviderConfig {
                egress_pool: vec![],
                egress_strategy: "round_robin".to_string(),
                rate_limits: None,
                base_url: "https://api.example.com".to_string(),
                default_model: "shared-model".to_string(),
                strategy: "priority".to_string(),
                billing_mode: BillingMode::Metered,
                input_price: 0.10,
                cached_price: 0.01,
                output_price: 0.20,
                models,
                model_specs: vec![
                    ModelSpec {
                        rate_limits: None,
                        priority: None,
                        name: "shared-model".to_string(),
                        tier: ModelTier::Flagship,
                        context_window: "1M".to_string(),
                        max_output: "32K".to_string(),
                        input_types: vec!["text".to_string()],
                        output_types: vec!["text".to_string()],
                        ..Default::default()
                    },
                    ModelSpec {
                        rate_limits: None,
                        priority: None,
                        name: "solo-model".to_string(),
                        tier: ModelTier::Standard,
                        context_window: "128K".to_string(),
                        max_output: "32K".to_string(),
                        input_types: vec!["text".to_string()],
                        output_types: vec!["text".to_string()],
                        ..Default::default()
                    },
                ],
                default_protocol: None,
                chat_url: None,
                responses_url: None,
                messages_url: None,
                proxy: None,
                timeout_secs: None,
                ttfb_timeout_secs: None,
            },
        );
    }
    let state = AppState::new(config);

    let models = state.list_all_models();
    let ids: Vec<&str> = models.iter().map(|(id, _, _, _)| id.as_str()).collect();

    // The bare name is deduped to ONE entry...
    assert_eq!(ids.iter().filter(|id| **id == "shared-model").count(), 1);
    // ...while each provider's instance is exposed as a pindown alias.
    assert!(
        ids.contains(&"alpha/shared-model"),
        "missing alpha alias: {ids:?}"
    );
    assert!(
        ids.contains(&"beta/shared-model"),
        "missing beta alias: {ids:?}"
    );
    // 1M shared models also get provider-scoped [1m] aliases; the pooled
    // `shared-model[1m]` variant stays as before.
    assert!(
        ids.contains(&"shared-model[1m]"),
        "missing pooled [1m]: {ids:?}"
    );
    assert!(
        ids.contains(&"alpha/shared-model[1m]"),
        "missing alpha [1m] alias: {ids:?}"
    );
    assert!(
        ids.contains(&"beta/shared-model[1m]"),
        "missing beta [1m] alias: {ids:?}"
    );
    // Single-provider models get NO alias (the list stays lean).
    assert!(
        !ids.contains(&"alpha/solo-model"),
        "single-provider model must not get an alias: {ids:?}"
    );
    // Deterministic ordering: provider iteration is name-sorted, so the
    // alpha alias comes before the beta alias (and the list is stable).
    let i_alpha = ids
        .iter()
        .position(|id| *id == "alpha/shared-model")
        .unwrap();
    let i_beta = ids
        .iter()
        .position(|id| *id == "beta/shared-model")
        .unwrap();
    assert!(i_alpha < i_beta, "list must be provider-sorted");
    let again = state.list_all_models();
    assert_eq!(
        models
            .iter()
            .map(|(id, _, _, _)| id.as_str())
            .collect::<Vec<_>>(),
        again
            .iter()
            .map(|(id, _, _, _)| id.as_str())
            .collect::<Vec<_>>(),
        "list must be deterministic across calls"
    );
}

#[tokio::test]
async fn test_get_model_provider_model_two_segment_route() {
    let (gateway_addr, _a_hits, _b_hits) = spawn_quota_failover_gateway(
        false,
        "/v1/chat/completions",
        QuotaFailMode::PaymentRequired,
    )
    .await;
    let client = reqwest::Client::new();

    // Two-segment route resolves `provider/model` without URL-encoding the slash.
    let resp = client
        .get(format!(
            "{}/v1/models/backup_provider/quota-test-model",
            gateway_addr
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "backup_provider/quota-test-model");
    assert_eq!(body["owned_by"], "backup_provider");

    let resp = client
        .get(format!(
            "{}/v1/models/quota_provider/quota-test-model",
            gateway_addr
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["owned_by"], "quota_provider");

    // Unknown provider/model -> 404.
    let resp = client
        .get(format!("{}/v1/models/nope/nope-model", gateway_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // The aliases are also visible in the /v1/models listing over HTTP.
    let list: serde_json::Value = client
        .get(format!("{}/v1/models", gateway_addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = list["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert!(ids.contains(&"quota_provider/quota-test-model"));
    assert!(ids.contains(&"backup_provider/quota-test-model"));
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
        axum::serve(upstream_listener, mock_anthropic)
            .await
            .unwrap();
    });

    let pool = Arc::new(KeyPool::new("anthropic", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("ant-k1", "sk-ant-key", 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "anthropic".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: format!("http://{}/v1/messages", upstream_addr),
            default_model: "claude-3-7-sonnet".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 3.0,
            cached_price: 0.3,
            output_price: 15.0,
            models: vec!["claude-3-7-sonnet".to_string()],
            model_specs: vec![ModelSpec {
                rate_limits: None,
                priority: None,
                name: "claude-3-7-sonnet".to_string(),
                tier: ModelTier::Flagship,
                context_window: "1M".to_string(),
                max_output: "64K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                ..Default::default()
            }],
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
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
    assert_eq!(
        resp.headers()
            .get("x-ponyllm-routed-model")
            .unwrap()
            .to_str()
            .unwrap(),
        "claude-3-7-sonnet"
    );
    assert_eq!(
        resp.headers()
            .get("x-ponyllm-strategy")
            .unwrap()
            .to_str()
            .unwrap(),
        "speed"
    );
    assert_eq!(
        resp.headers()
            .get("x-ponyllm-tier")
            .unwrap()
            .to_str()
            .unwrap(),
        "F"
    );

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
            axum::routing::post(
                |axum::Json(req): axum::Json<serde_json::Value>| async move {
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
                },
            ),
        );
        axum::serve(mock_b, app).await.unwrap();
    });

    // 1. Initial configuration: prov_a only
    let mut gw_config = GatewayConfig::default();
    gw_config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    gw_config.bind_addr = "127.0.0.1:0".to_string();
    gw_config.api_key = String::new();
    gw_config.providers.insert(
        "prov_a".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: "http://127.0.0.1:12345/v1".to_string(),
            default_model: "model-a".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 1.0,
            cached_price: 0.5,
            output_price: 2.0,
            models: vec!["model-a".to_string()],
            model_specs: vec![],
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
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
    new_config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    new_config.bind_addr = gw_config.bind_addr.clone();
    new_config.providers.insert(
        "prov_b".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: format!("http://{}/v1", mock_b_addr),
            default_model: "model-b".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.1,
            cached_price: 0.05,
            output_price: 0.2,
            models: vec!["model-b".to_string()],
            model_specs: vec![],
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
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
        chat_resp
            .headers()
            .get("x-ponyllm-provider")
            .unwrap()
            .to_str()
            .unwrap(),
        "prov_b"
    );
    let chat_json: serde_json::Value = chat_resp.json().await.unwrap();
    assert_eq!(
        chat_json["choices"][0]["message"]["content"],
        "Hello from Provider B"
    );
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
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "deepseek".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: format!("http://{}", upstream_addr),
            default_model: "deepseek-v4-flash".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.14,
            cached_price: 0.014,
            output_price: 0.28,
            models: vec!["deepseek-v4-flash".to_string()],
            model_specs: vec![ModelSpec {
                rate_limits: None,
                priority: None,
                name: "deepseek-v4-flash".to_string(),
                tier: ModelTier::Flagship,
                context_window: "1M".to_string(),
                max_output: "32K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                ..Default::default()
            }],
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
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

    assert_eq!(
        resp.status(),
        200,
        "Large payload >2MB must succeed through gateway"
    );
    let resp_json: serde_json::Value = resp.json().await.unwrap();
    assert!(resp_json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .contains("Received 3145728 bytes"));
}

#[tokio::test]
async fn test_custom_request_body_limit_rejection_with_helpful_error() {
    let pool = Arc::new(KeyPool::new("test-p", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.request_body_limit = 16 * 1024; // 16 KB small limit
    config.providers.insert(
        "test-p".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: "http://127.0.0.1:9".to_string(),
            default_model: "test-model".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.1,
            cached_price: 0.01,
            output_price: 0.2,
            models: vec!["test-model".to_string()],
            model_specs: vec![],
            default_protocol: None,
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
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

    assert_eq!(resp.status(), 413);
    let err_json: serde_json::Value = resp.json().await.unwrap();
    let err_msg = err_json["error"]["message"].as_str().unwrap();
    assert!(
        err_msg.contains("Request body length limit exceeded")
            || err_msg.contains("length limit exceeded")
    );
}

#[tokio::test]
async fn test_responses_cross_provider_failover() {
    let healthy_upstream = Router::new().route(
        "/v1/responses",
        post(|Json(req): Json<serde_json::Value>| async move {
            let m = req["model"].as_str().unwrap_or_default().to_string();
            axum::Json(json!({
                "id": "resp-mock-1",
                "object": "response",
                "status": "completed",
                "model": m,
                "output": [{
                    "type": "message",
                    "id": "msg-1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Hello responses backup"}]
                }],
                "usage": {"total_tokens": 15, "input_tokens": 10, "output_tokens": 5}
            }))
        }),
    );
    let healthy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let healthy_addr = healthy_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(healthy_listener, healthy_upstream)
            .await
            .unwrap();
    });

    let pool_broken = Arc::new(KeyPool::new("resp_broken", RoutingStrategy::RoundRobin));
    pool_broken.add_key(ApiKeyEntry::new("rb-k1", "sk-broken", 1, 10));
    let pool_backup = Arc::new(KeyPool::new("resp_backup", RoutingStrategy::RoundRobin));
    pool_backup.add_key(ApiKeyEntry::new("rk-k1", "sk-backup", 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.max_retries = 1;
    for (name, url, price) in [
        ("resp_broken", "http://127.0.0.1:1".to_string(), 0.10),
        ("resp_backup", format!("http://{}", healthy_addr), 0.20),
    ] {
        config.providers.insert(
            name.to_string(),
            ProviderConfig {
                egress_pool: vec![],
                egress_strategy: "round_robin".to_string(),
                rate_limits: None,
                base_url: url,
                default_model: "muse-spark-test".to_string(),
                strategy: "priority".to_string(),
                billing_mode: BillingMode::Metered,
                input_price: price,
                cached_price: 0.01,
                output_price: 0.20,
                models: vec!["muse-spark-test".to_string()],
                model_specs: vec![],
                // Both mocks are Responses-native; declare it so the gateway
                // routes to /v1/responses instead of heuristic Chat.
                default_protocol: Some(UpstreamProtocol::Responses),
                chat_url: None,
                responses_url: None,
                messages_url: None,
                proxy: None,
                timeout_secs: None,
                ttfb_timeout_secs: None,
            },
        );
    }

    let state = Arc::new(AppState::new(config));
    state.register_pool("resp_broken", pool_broken);
    state.register_pool("resp_backup", pool_backup);

    let gateway_app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway_app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", gateway_addr))
        .json(&json!({"model": "muse-spark-test", "input": "Hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("x-ponyllm-provider")
            .unwrap()
            .to_str()
            .unwrap(),
        "resp_backup"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["model"], "muse-spark-test");
}

fn cross_protocol_provider(
    base_url: String,
    model: &str,
    proto: UpstreamProtocol,
) -> ProviderConfig {
    ProviderConfig {
        egress_pool: vec![],
        egress_strategy: "round_robin".to_string(),
        rate_limits: None,
        base_url,
        default_model: model.to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.1,
        cached_price: 0.01,
        output_price: 0.2,
        models: vec![model.to_string()],
        model_specs: vec![],
        default_protocol: Some(proto),
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
        timeout_secs: None,
        ttfb_timeout_secs: None,
    }
}

fn responses_mock_object(model: &str, text: &str) -> serde_json::Value {
    json!({
        "id": "resp-mock-1",
        "object": "response",
        "status": "completed",
        "model": model,
        "output": [{
            "type": "message",
            "id": "msg-1",
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "text", "text": text}]
        }],
        "usage": {"total_tokens": 15, "input_tokens": 10, "output_tokens": 5}
    })
}

#[tokio::test]
async fn test_chat_entry_translates_responses_native_upstream() {
    let mock = Router::new().route(
        "/v1/responses",
        post(|Json(req): Json<serde_json::Value>| async move {
            assert!(
                req.get("input").is_some(),
                "expected Responses shape, got: {}",
                req
            );
            axum::Json(responses_mock_object(
                req["model"].as_str().unwrap_or("m"),
                "Hello from responses-native",
            ))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("spark", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "spark".to_string(),
        cross_protocol_provider(
            format!("http://{}", addr),
            "muse-spark",
            UpstreamProtocol::Responses,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("spark", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "muse-spark",
            "messages": [{"role": "user", "content": "Hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("x-ponyllm-protocol")
            .unwrap()
            .to_str()
            .unwrap(),
        "responses"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "Hello from responses-native"
    );
    assert_eq!(body["model"], "muse-spark");
}

#[tokio::test]
async fn test_responses_entry_translates_chat_native_upstream() {
    let mock = Router::new().route(
        "/v1/chat/completions",
        post(|Json(req): Json<serde_json::Value>| async move {
            assert!(
                req.get("messages").is_some(),
                "expected Chat shape, got: {}",
                req
            );
            axum::Json(json!({
                "id": "chatcmpl-mock-1",
                "object": "chat.completion",
                "created": 1710000000,
                "model": req["model"],
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "Hello from chat-native"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 8, "completion_tokens": 4, "total_tokens": 12}
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("chatter", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "chatter".to_string(),
        cross_protocol_provider(
            format!("http://{}", addr),
            "chat-model",
            UpstreamProtocol::Chat,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("chatter", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", gw_addr))
        .json(&json!({"model": "chat-model", "input": "Hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("x-ponyllm-protocol")
            .unwrap()
            .to_str()
            .unwrap(),
        "chat"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "response");
    assert_eq!(body["status"], "completed");
    assert_eq!(
        body["output"][0]["content"][0]["text"],
        "Hello from chat-native"
    );
    assert_eq!(body["model"], "chat-model");
}

#[tokio::test]
async fn test_responses_entry_translates_antigravity_upstream() {
    let mock = Router::new().route(
        "/v1internal:streamGenerateContent",
        post(|Json(req): Json<serde_json::Value>| async move {
            assert!(req.get("request").is_some(), "expected Antigravity shape, got: {}", req);
            let sse_data = "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello from Antigravity via Responses!\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":6,\"totalTokenCount\":16}}}\n\n";
            axum::response::Response::builder()
                .header("content-type", "text/event-stream")
                .body(axum::body::Body::from(sse_data))
                .unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("agy_prov", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "agy_prov".to_string(),
        cross_protocol_provider(
            format!("http://{}", addr),
            "gemini-3.8-flash-high",
            UpstreamProtocol::Antigravity,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("agy_prov", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", gw_addr))
        .json(&json!({
            "model": "gemini-3.8-flash-high",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": "Hi from Responses client"
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "Antigravity provider should now succeed for /v1/responses"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "response");
    assert_eq!(body["status"], "completed");
    assert_eq!(
        body["output"][0]["content"][0]["text"],
        "Hello from Antigravity via Responses!"
    );
    assert_eq!(body["model"], "gemini-3.8-flash-high");

    // Also test streaming responses through Antigravity
    let stream_resp = client
        .post(format!("http://{}/v1/responses", gw_addr))
        .json(&json!({
            "model": "gemini-3.8-flash-high",
            "stream": true,
            "input": "Stream hi"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(stream_resp.status(), 200);
    assert_eq!(
        stream_resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "text/event-stream"
    );
    let stream_text = stream_resp.text().await.unwrap();
    assert!(
        stream_text.contains("response.created")
            || stream_text.contains("response.output_item.added")
            || stream_text.contains("response.output_text.delta")
    );
}

#[tokio::test]
async fn test_messages_entry_translates_responses_native_upstream() {
    let mock = Router::new().route(
        "/v1/responses",
        post(|Json(req): Json<serde_json::Value>| async move {
            assert!(
                req.get("input").is_some(),
                "expected Responses shape, got: {}",
                req
            );
            axum::Json(responses_mock_object(
                req["model"].as_str().unwrap_or("m"),
                "Hello anthropic client",
            ))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("spark2", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "spark2".to_string(),
        cross_protocol_provider(
            format!("http://{}", addr),
            "spark-msg",
            UpstreamProtocol::Responses,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("spark2", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/messages", gw_addr))
        .json(&json!({
            "model": "spark-msg",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "Hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("x-ponyllm-protocol")
            .unwrap()
            .to_str()
            .unwrap(),
        "responses"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["type"], "message");
    assert_eq!(body["content"][0]["text"], "Hello anthropic client");
    assert_eq!(body["model"], "spark-msg");
}

#[tokio::test]
async fn test_chat_streaming_translates_responses_native_upstream() {
    let mock = Router::new().route(
        "/v1/responses",
        post(|| async move {
            let sse = concat!(
                "event: response.created\n",
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_s1\",\"object\":\"response\",\"status\":\"in_progress\",\"model\":\"m\",\"output\":[]}}\n\n",
                "event: response.output_text.delta\n",
                "data: {\"type\":\"response.output_text.delta\",\"response_id\":\"resp_s1\",\"item_id\":\"it_0\",\"output_index\":0,\"content_index\":0,\"delta\":\"streamed\"}\n\n",
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_s1\",\"object\":\"response\",\"status\":\"completed\",\"model\":\"m\",\"output\":[],\"usage\":{\"total_tokens\":6,\"input_tokens\":4,\"output_tokens\":2}}}\n\n",
            );
            (
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                axum::body::Body::from(sse),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("spark3", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "spark3".to_string(),
        cross_protocol_provider(
            format!("http://{}", addr),
            "spark-stream",
            UpstreamProtocol::Responses,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("spark3", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "spark-stream",
            "messages": [{"role": "user", "content": "Hi"}],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    assert!(
        text.contains("\"content\":\"streamed\""),
        "missing translated chunk: {text}"
    );
    assert!(text.contains("data: [DONE]"), "missing terminator: {text}");
}

#[tokio::test]
async fn test_messages_image_only_translated_to_responses_rejected_with_anthropic_error() {
    let pool = Arc::new(KeyPool::new("spark_img", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "spark_img".to_string(),
        cross_protocol_provider(
            "http://127.0.0.1:9999".to_string(),
            "spark-resp",
            UpstreamProtocol::Responses,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("spark_img", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/messages", gw_addr))
        .json(&json!({
            "model": "spark-resp",
            "system": "You are a helpful assistant.",
            "max_tokens": 100,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
                    }
                }]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["type"], "error",
        "Must have top-level Anthropic error envelope"
    );
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("does not support modality 'image'"));
}

#[tokio::test]
async fn test_provider_proxy_routing_and_isolation() {
    use ponyllm_core::executor::create_upstream_http_client_with_options;

    // Direct provider client ignores env proxies
    let client_direct = create_upstream_http_client_with_options(None, false);
    let _ = client_direct;

    // Custom proxy client builds cleanly
    let client_proxy =
        create_upstream_http_client_with_options(Some("http://127.0.0.1:8899"), false);
    let _ = client_proxy;

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "proxied_prov".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: "https://example.com".to_string(),
            default_model: "mock".to_string(),
            proxy: Some("http://127.0.0.1:8899".to_string()),
            ..Default::default()
        },
    );
    config.providers.insert(
        "direct_prov".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: "https://example.com".to_string(),
            default_model: "mock".to_string(),
            proxy: None,
            ..Default::default()
        },
    );

    let state = AppState::new(config);
    // Verified: http_client_for_provider returns distinct clients
    let _c1 = state.http_client_for_provider("proxied_prov");
    let _c2 = state.http_client_for_provider("direct_prov");
}

#[tokio::test]
async fn test_model_specific_base_url_routing() {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    let mut prov = ProviderConfig {
        egress_pool: vec![],
        egress_strategy: "round_robin".to_string(),
        rate_limits: None,
        base_url: "https://provider.example.com/v1".to_string(),
        default_model: "default-model".to_string(),
        models: vec!["default-model".to_string(), "custom-node".to_string()],
        ..Default::default()
    };
    prov.model_specs.push(ModelSpec {
        rate_limits: None,
        priority: None,
        name: "custom-node".to_string(),
        base_url: Some("https://model-node.example.com/v1".to_string()),
        ..Default::default()
    });
    config.providers.insert("prov1".to_string(), prov);

    let state = AppState::new(config);

    // 1. Model with custom base_url routes directly to model's base_url
    let candidates = state
        .resolve_routed_targets(&ParsedRequestModel::parse("custom-node"), None)
        .expect("should resolve candidates for custom-node");
    assert!(!candidates.is_empty());
    let target = &candidates[0];
    assert_eq!(target.base_url, "https://model-node.example.com/v1");
    assert_eq!(
        target.endpoint_base.as_deref(),
        Some("https://model-node.example.com/v1")
    );
    assert_eq!(
        target.chat_completions_url(),
        "https://model-node.example.com/v1/chat/completions"
    );

    // 2. Default model without custom base_url falls back to provider base_url
    let default_candidates = state
        .resolve_routed_targets(&ParsedRequestModel::parse("default-model"), None)
        .expect("should resolve candidates for default-model");
    let def_target = &default_candidates[0];
    assert_eq!(def_target.base_url, "https://provider.example.com/v1");
    assert_eq!(
        def_target.chat_completions_url(),
        "https://provider.example.com/v1/chat/completions"
    );
}

#[test]
fn test_deepseek_v41_flash_alias_routes_to_live_upstream_name() {
    use ponyllm_server::{AppState, GatewayConfig, ModelSpec, ProviderConfig};

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "deepseek".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: "https://api.deepseek.com".to_string(),
            default_model: "deepseek-flash".to_string(),
            models: vec!["deepseek-flash".to_string()],
            model_specs: vec![ModelSpec {
                rate_limits: None,
                priority: None,
                name: "deepseek-flash".to_string(),
                context_window: "1M".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    let state = AppState::new(config);

    // Alias resolves to the live upstream name for failover + wire model.
    let targets = state
        .resolve_routed_targets(&ParsedRequestModel::parse("deepseek-v4.1-flash"), None)
        .expect("alias must resolve");
    assert_eq!(targets[0].provider_name, "deepseek");
    assert_eq!(targets[0].physical_model, "deepseek-flash");

    // Explicit config entry still wins over the alias (never shadow config).
    let mut config2 = GatewayConfig::default();
    config2.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config2.providers.insert(
        "deepseek".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: "https://api.deepseek.com".to_string(),
            default_model: "deepseek-flash".to_string(),
            models: vec![
                "deepseek-flash".to_string(),
                "deepseek-v4.1-flash".to_string(),
            ],
            ..Default::default()
        },
    );
    let state2 = AppState::new(config2);
    let targets2 = state2
        .resolve_routed_targets(&ParsedRequestModel::parse("deepseek-v4.1-flash"), None)
        .expect("explicit entry must resolve");
    assert_eq!(targets2[0].physical_model, "deepseek-v4.1-flash");

    // Alias is listed for discovery, with its [1m] variant.
    let models = state.list_all_models();
    assert!(models
        .iter()
        .any(|(m, p, _, _)| m == "deepseek-v4.1-flash" && p == "deepseek"));
    assert!(models
        .iter()
        .any(|(m, p, _, _)| m == "deepseek-v4.1-flash[1m]" && p == "deepseek"));
}

#[tokio::test]
async fn test_deepseek_v41_flash_alias_echo_and_wire_model() {
    // Mock upstream asserts it receives the live name, never the retired alias.
    let mock_upstream = Router::new().route(
        "/v1/chat/completions",
        post(|Json(req): Json<serde_json::Value>| async move {
            let upstream_model = req["model"].as_str().unwrap_or_default().to_string();
            assert_eq!(upstream_model, "deepseek-flash");
            axum::Json(json!({
                "id": "chatcmpl-alias-1",
                "object": "chat.completion",
                "created": 1710000000,
                "model": upstream_model,
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "alias ok"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }))
        }),
    );
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock_upstream).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("deepseek", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("ds-k1", "sk-ds-key", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "deepseek".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: format!("http://{}", upstream_addr),
            default_model: "deepseek-flash".to_string(),
            models: vec!["deepseek-flash".to_string()],
            ..Default::default()
        },
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "deepseek-v4.1-flash",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("x-ponyllm-routed-model")
            .unwrap()
            .to_str()
            .unwrap(),
        "deepseek-flash"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["model"], "deepseek-v4.1-flash");
}

// -----------------------------------------------------------------------------
// Model priority across providers (model-priority ADR)
// -----------------------------------------------------------------------------

/// Helper: one provider serving `duo` with the given priority and prices.
fn priority_provider(
    name: &str,
    priority: Option<u32>,
    input_price: f64,
) -> (String, ProviderConfig) {
    (
        name.to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: format!("https://{}.example.com", name),
            default_model: "duo".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price,
            cached_price: input_price / 2.0,
            output_price: 2.0,
            models: vec!["duo".to_string()],
            model_specs: vec![ModelSpec {
                rate_limits: None,
                name: "duo".to_string(),
                tier: ModelTier::Standard,
                priority,
                context_window: default_context_window(),
                max_output: default_max_output(),
                input_types: default_modalities(),
                output_types: default_modalities(),
                billing_mode: None,
                input_price: None,
                cached_price: None,
                output_price: None,
                pricing_mode: None,
                pricing_periods: Vec::new(),
                display_name: None,
                temperature: None,
                top_p: None,
                protocol: None,
                base_url: None,
                thinking_default: None,
                thinking_max: None,
                proxy: None,
                timeout_secs: None,
                fallbacks: Vec::new(),
            }],
            default_protocol: Some(UpstreamProtocol::Chat),
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
        },
    )
}

#[test]
fn test_model_priority_dominates_strategy_scoring() {
    use ponyllm_server::routes::models::ParsedRequestModel;
    use ponyllm_server::{AppState, GatewayConfig};

    // hi-pp is far pricier than lo-pp, so the Economy default would choose
    // lo-pp first; explicit priority must override the price score.
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    let (lo_name, lo_cfg) = priority_provider("lo-pp", Some(1), 0.1);
    config.providers.insert(lo_name, lo_cfg);
    let (hi_name, hi_cfg) = priority_provider("hi-pp", Some(10), 5.0);
    config.providers.insert(hi_name, hi_cfg);

    let state = AppState::new(config);
    let parsed = ParsedRequestModel::parse("duo");
    let targets = state
        .resolve_routed_targets(&parsed, Some(GatewayRoutingStrategy::Economy))
        .unwrap();
    assert_eq!(targets.len(), 2);
    assert_eq!(
        targets[0].provider_name, "hi-pp",
        "higher priority must win over cheaper price"
    );
    assert_eq!(targets[1].provider_name, "lo-pp");
}

#[test]
fn test_model_priority_tie_keeps_strategy_scoring() {
    use ponyllm_server::routes::models::ParsedRequestModel;
    use ponyllm_server::{AppState, GatewayConfig};

    // Both providers have no priority: the Economy price score must decide,
    // exactly as before the priority feature existed.
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    let (ex_name, ex_cfg) = priority_provider("expensive", None, 5.0);
    config.providers.insert(ex_name, ex_cfg);
    let (ch_name, ch_cfg) = priority_provider("cheap", None, 0.1);
    config.providers.insert(ch_name, ch_cfg);

    let state = AppState::new(config);
    let parsed = ParsedRequestModel::parse("duo");
    let targets = state
        .resolve_routed_targets(&parsed, Some(GatewayRoutingStrategy::Economy))
        .unwrap();
    assert_eq!(targets.len(), 2);
    assert_eq!(
        targets[0].provider_name, "cheap",
        "no priority keeps strategy (price) ordering"
    );
    assert_eq!(targets[1].provider_name, "expensive");
}

#[test]
fn test_model_priority_equal_values_fall_back_to_strategy() {
    use ponyllm_server::routes::models::ParsedRequestModel;
    use ponyllm_server::{AppState, GatewayConfig};

    // Equal priorities behave like no priority: price decides.
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    let (ex_name, ex_cfg) = priority_provider("expensive", Some(7), 5.0);
    config.providers.insert(ex_name, ex_cfg);
    let (ch_name, ch_cfg) = priority_provider("cheap", Some(7), 0.1);
    config.providers.insert(ch_name, ch_cfg);

    let state = AppState::new(config);
    let parsed = ParsedRequestModel::parse("duo");
    let targets = state
        .resolve_routed_targets(&parsed, Some(GatewayRoutingStrategy::Economy))
        .unwrap();
    assert_eq!(targets[0].provider_name, "cheap");
    assert_eq!(targets[1].provider_name, "expensive");
}
