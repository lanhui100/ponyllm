use ponyllm_core::pool::*;
use ponyllm_server::{AppState, GatewayConfig, ProviderConfig, ModelSpec};
use ponyllm_server::routes::models::ParsedRequestModel;
use std::collections::HashMap;

#[test]
fn test_node_latency_metrics_dynamic_update_and_speed_scoring() {
    let mut providers = HashMap::new();

    let p_fast = ProviderConfig {
        base_url: "https://fast.example.com".to_string(),
        default_model: "test-model".to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 1.0,
        cached_price: 0.5,
        output_price: 2.0,
        models: vec!["test-model".to_string()],
        model_specs: vec![ModelSpec {
            name: "test-model".to_string(),
            tier: ModelTier::Standard,
            context_window: "128K".to_string(),
            max_output: "8K".to_string(),
            input_types: vec!["text".to_string()],
            output_types: vec!["text".to_string()],
            billing_mode: None,
            input_price: None,
            cached_price: None,
            output_price: None,
        }],
    };

    let p_slow = ProviderConfig {
        base_url: "https://slow.example.com".to_string(),
        default_model: "test-model".to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 1.0,
        cached_price: 0.5,
        output_price: 2.0,
        models: vec!["test-model".to_string()],
        model_specs: vec![ModelSpec {
            name: "test-model".to_string(),
            tier: ModelTier::Standard,
            context_window: "128K".to_string(),
            max_output: "8K".to_string(),
            input_types: vec!["text".to_string()],
            output_types: vec!["text".to_string()],
            billing_mode: None,
            input_price: None,
            cached_price: None,
            output_price: None,
        }],
    };

    providers.insert("fast_node".to_string(), p_fast);
    providers.insert("slow_node".to_string(), p_slow);

    let gw_config = GatewayConfig {
        default_strategy: GatewayRoutingStrategy::Speed,
        providers,
        ..Default::default()
    };

    let state = AppState::new(gw_config);

    // Initial state: both are cold defaults (TTFT=800ms, TPS=40)
    let fast_m = state.get_or_create_node_metrics("fast_node");
    let slow_m = state.get_or_create_node_metrics("slow_node");
    assert_eq!(fast_m.get_ttft_ms(), 800.0);
    assert_eq!(slow_m.get_ttft_ms(), 800.0);

    // Now simulate dynamic metric feedback from actual requests:
    // fast_node: TTFT 120ms, TPS 120
    fast_m.update(Some(120.0), Some(120.0), false);
    fast_m.update(Some(110.0), Some(130.0), false);

    // slow_node: TTFT 1800ms, TPS 15
    slow_m.update(Some(1800.0), Some(15.0), false);
    slow_m.update(Some(1900.0), Some(12.0), false);

    // Metrics must have drifted from cold defaults via EWMA
    assert!(fast_m.get_ttft_ms() < 600.0);
    assert!(fast_m.get_tps() > 50.0);
    assert!(slow_m.get_ttft_ms() > 1000.0);
    assert!(slow_m.get_tps() < 35.0);

    // Resolve with Speed strategy: fast_node MUST be ranked first!
    let parsed = ParsedRequestModel::parse("test-model");
    let targets = state.resolve_routed_targets(&parsed, Some(GatewayRoutingStrategy::Speed)).unwrap();

    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].provider_name, "fast_node");
    assert_eq!(targets[1].provider_name, "slow_node");
}
