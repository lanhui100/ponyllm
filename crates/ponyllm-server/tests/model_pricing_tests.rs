use ponyllm_core::pool::*;
use ponyllm_server::{AppState, GatewayConfig, ProviderConfig, ModelSpec};
use ponyllm_server::routes::models::ParsedRequestModel;
use std::collections::HashMap;

#[test]
fn test_model_spec_pricing_inheritance_and_override() {
    let provider = ProviderConfig {
        base_url: "https://api.deepseek.com".to_string(),
        default_model: "deepseek-chat".to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.14,
        cached_price: 0.014,
        output_price: 0.28,
        models: vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()],
        model_specs: vec![
            // deepseek-chat inherits provider default pricing
            ModelSpec {
                name: "deepseek-chat".to_string(),
                tier: ModelTier::Standard,
                context_window: "128K".to_string(),
                max_output: "8K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                ..Default::default()
            },
            // deepseek-reasoner has higher custom pricing with special cached price
            ModelSpec {
                name: "deepseek-reasoner".to_string(),
                tier: ModelTier::Flagship,
                context_window: "128K".to_string(),
                max_output: "8K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                input_price: Some(0.55),
                cached_price: Some(0.14),
                output_price: Some(2.19),
                ..Default::default()
            },
        ],
        default_protocol: None,
        chat_url: None,
        responses_url: None,
        messages_url: None,
    };

    // 1. deepseek-chat should inherit provider default
    let chat_pricing = provider.get_model_pricing("deepseek-chat");
    assert_eq!(chat_pricing.input_price, 0.14);
    assert_eq!(chat_pricing.cached_price, 0.014);
    assert_eq!(chat_pricing.output_price, 0.28);

    // 2. deepseek-reasoner should use its own custom pricing
    let r1_pricing = provider.get_model_pricing("deepseek-reasoner");
    assert_eq!(r1_pricing.input_price, 0.55);
    assert_eq!(r1_pricing.cached_price, 0.14);
    assert_eq!(r1_pricing.output_price, 2.19);

    // 3. Unconfigured model should fallback to provider default
    let unconfigured_pricing = provider.get_model_pricing("non-configured-model");
    assert_eq!(unconfigured_pricing.input_price, 0.14);
    assert_eq!(unconfigured_pricing.cached_price, 0.014);
    assert_eq!(unconfigured_pricing.output_price, 0.28);
}

#[test]
fn test_economy_routing_respects_model_level_pricing() {
    // Two providers providing the same model "special-model", but with different model-level pricing
    let mut providers = HashMap::new();

    // Provider A: default is cheap (0.1), but special-model is expensive (2.0)
    let p_a = ProviderConfig {
        base_url: "https://api.a.com".to_string(),
        default_model: "special-model".to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.10,
        cached_price: 0.05,
        output_price: 0.20,
        models: vec!["special-model".to_string()],
        model_specs: vec![
            ModelSpec {
                name: "special-model".to_string(),
                tier: ModelTier::Standard,
                context_window: "128K".to_string(),
                max_output: "8K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                input_price: Some(2.0),
                cached_price: Some(1.0),
                output_price: Some(4.0),
                ..Default::default()
            }
        ],
        default_protocol: None,
        chat_url: None,
        responses_url: None,
        messages_url: None,
    };

    // Provider B: default is expensive (1.0), but special-model is discounted (0.3)
    let p_b = ProviderConfig {
        base_url: "https://api.b.com".to_string(),
        default_model: "special-model".to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 1.0,
        cached_price: 0.5,
        output_price: 2.0,
        models: vec!["special-model".to_string()],
        model_specs: vec![
            ModelSpec {
                name: "special-model".to_string(),
                tier: ModelTier::Standard,
                context_window: "128K".to_string(),
                max_output: "8K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                input_price: Some(0.3),
                cached_price: Some(0.05),
                output_price: Some(0.6),
                ..Default::default()
            }
        ],
        default_protocol: None,
        chat_url: None,
        responses_url: None,
        messages_url: None,
    };

    providers.insert("provider_a".to_string(), p_a);
    providers.insert("provider_b".to_string(), p_b);

    let gw_config = GatewayConfig {
        default_strategy: GatewayRoutingStrategy::Economy,
        providers,
        ..Default::default()
    };

    let state = AppState::new(gw_config);
    let parsed = ParsedRequestModel::parse("special-model");
    let targets = state.resolve_routed_targets(&parsed, Some(GatewayRoutingStrategy::Economy)).unwrap();

    assert_eq!(targets.len(), 2);
    // Because Provider B offers special-model at 0.3 vs Provider A at 2.0,
    // Provider B MUST be ranked first, even though Provider A has a lower provider-level default price!
    assert_eq!(targets[0].provider_name, "provider_b");
    assert_eq!(targets[1].provider_name, "provider_a");
}

#[test]
fn test_pricing_anti_inversion_and_free_model_preservation() {
    let p = ProviderConfig {
        base_url: "https://api.example.com".to_string(),
        default_model: "base".to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 2.50,
        cached_price: 1.25, // 50% discount
        output_price: 10.00,
        models: vec!["mini".to_string(), "free-trial".to_string()],
        model_specs: vec![
            // mini only specifies input_price = 0.15; cached_price should scale down to 0.075, not 1.25!
            ModelSpec {
                name: "mini".to_string(),
                tier: ModelTier::Light,
                context_window: "128K".to_string(),
                max_output: "4K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                input_price: Some(0.15),
                ..Default::default()
            },
            // free-trial has input_price = 0.0; cached_price must be 0.0, output inherits or is 0
            ModelSpec {
                name: "free-trial".to_string(),
                tier: ModelTier::Light,
                context_window: "32K".to_string(),
                max_output: "2K".to_string(),
                input_types: vec!["text".to_string()],
                output_types: vec!["text".to_string()],
                input_price: Some(0.0),
                output_price: Some(0.0),
                ..Default::default()
            },
        ],
        default_protocol: None,
        chat_url: None,
        responses_url: None,
        messages_url: None,
    };

    let mini_pricing = p.get_model_pricing("mini");
    assert_eq!(mini_pricing.input_price, 0.15);
    // Cached price is scaled by 50% ratio (0.15 * 0.5 = 0.075), never exceeding input_price!
    assert!(mini_pricing.cached_price <= mini_pricing.input_price);
    assert!((mini_pricing.cached_price - 0.075).abs() < 1e-6);

    let free_pricing = p.get_model_pricing("free-trial");
    assert_eq!(free_pricing.input_price, 0.0);
    assert_eq!(free_pricing.cached_price, 0.0);
    assert_eq!(free_pricing.output_price, 0.0);
    assert!(free_pricing.is_free());
}

#[test]
fn test_anthropic_usage_extraction_includes_cached_tokens() {
    use ponyllm_server::streaming::extract_usage_tokens;

    let ant_usage = serde_json::json!({
        "usage": {
            "input_tokens": 100,
            "cache_read_input_tokens": 8000,
            "cache_creation_input_tokens": 500,
            "output_tokens": 250
        }
    });

    let (prompt, completion) = extract_usage_tokens(&ant_usage);
    assert_eq!(prompt, 8600); // 100 + 8000 + 500
    assert_eq!(completion, 250);
}

#[test]
fn test_hot_cache_probe_guides_economy_routing() {
    let mut providers = HashMap::new();
    let prompt_str = "System: Long code repository prompt context for testing prefix cache hit. ".repeat(20);
    let prompt = prompt_str.as_str();

    // Provider 1: Standard price $1.00, cached $0.10
    let p1 = ProviderConfig {
        base_url: "https://api.p1.com".to_string(),
        default_model: "chat".to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 1.00,
        cached_price: 0.10,
        output_price: 2.00,
        models: vec!["chat".to_string()],
        model_specs: vec![],
        default_protocol: None,
        chat_url: None,
        responses_url: None,
        messages_url: None,
    };

    // Provider 2: Standard price $0.80, cached $0.40
    let p2 = ProviderConfig {
        base_url: "https://api.p2.com".to_string(),
        default_model: "chat".to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.80,
        cached_price: 0.40,
        output_price: 2.00,
        models: vec!["chat".to_string()],
        model_specs: vec![],
        default_protocol: None,
        chat_url: None,
        responses_url: None,
        messages_url: None,
    };

    providers.insert("p1".to_string(), p1);
    providers.insert("p2".to_string(), p2);

    let gw_config = GatewayConfig {
        default_strategy: GatewayRoutingStrategy::Economy,
        providers,
        ..Default::default()
    };

    let state = AppState::new(gw_config);
    let parsed = ParsedRequestModel::parse("chat");

    // Case 1: Cold cache (no hot cache recorded). p2 is cheaper without cache (0.80 < 1.00)
    let cold_targets = state.resolve_routed_targets_with_prompt(&parsed, None, Some(prompt)).unwrap();
    assert_eq!(cold_targets[0].provider_name, "p2");

    // Case 2: Record that p1 dispatched this prompt earlier (now hot in p1)
    state.hot_cache.record_dispatch(prompt, "p1");

    // Now resolve with same prompt: p1's cached price ($0.10) beats p2's normal price ($0.80)!
    let hot_targets = state.resolve_routed_targets_with_prompt(&parsed, None, Some(prompt)).unwrap();
    assert_eq!(hot_targets[0].provider_name, "p1");
}
