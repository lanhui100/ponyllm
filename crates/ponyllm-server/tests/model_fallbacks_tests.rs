use ponyllm_core::pool::GatewayRoutingStrategy;
use ponyllm_server::routes::models::ParsedRequestModel;
use ponyllm_server::{AppState, GatewayConfig, ModelSpec, ProviderConfig};

#[tokio::test]
async fn test_model_fallbacks_routing() {
    let mut config = GatewayConfig::default();
    let prov = ProviderConfig {
        base_url: "https://example.com".to_string(),
        default_model: "gemini-3.8-flash-high".to_string(),
        strategy: "latency".to_string(),
        models: vec![
            "gemini-3.8-flash-high".to_string(),
            "gemini-3.8-flash-medium".to_string(),
        ],
        model_specs: vec![
            ModelSpec {
                name: "gemini-3.8-flash-high".to_string(),
                fallbacks: vec!["gemini-3.8-flash-medium".to_string()],
                ..ModelSpec::default()
            },
            ModelSpec {
                name: "gemini-3.8-flash-medium".to_string(),
                ..ModelSpec::default()
            },
        ],
        ..ProviderConfig::default()
    };

    config.providers.insert("antigravity".to_string(), prov);
    let state = AppState::new(config);

    let parsed = ParsedRequestModel::parse("gemini-3.8-flash-high");
    let targets = state.resolve_routed_targets(
        &parsed,
        Some(GatewayRoutingStrategy::Speed),
    ).unwrap();

    assert_eq!(targets.len(), 2, "Expected 2 targets (primary + fallback)");
    assert_eq!(targets[0].physical_model, "gemini-3.8-flash-high");
    assert_eq!(targets[1].physical_model, "gemini-3.8-flash-medium");
}

#[tokio::test]
async fn test_model_fallbacks_chain_and_cycle_prevention() {
    let mut config = GatewayConfig::default();
    let prov = ProviderConfig {
        base_url: "https://example.com".to_string(),
        default_model: "model-a".to_string(),
        strategy: "latency".to_string(),
        models: vec![
            "model-a".to_string(),
            "model-b".to_string(),
            "model-c".to_string(),
        ],
        model_specs: vec![
            ModelSpec {
                name: "model-a".to_string(),
                fallbacks: vec!["model-b".to_string()],
                ..ModelSpec::default()
            },
            ModelSpec {
                name: "model-b".to_string(),
                fallbacks: vec!["model-c".to_string(), "model-a".to_string()], // cyclic reference back to model-a
                ..ModelSpec::default()
            },
            ModelSpec {
                name: "model-c".to_string(),
                ..ModelSpec::default()
            },
        ],
        ..ProviderConfig::default()
    };

    config.providers.insert("mock".to_string(), prov);
    let state = AppState::new(config);

    let parsed = ParsedRequestModel::parse("model-a");
    let targets = state.resolve_routed_targets(
        &parsed,
        Some(GatewayRoutingStrategy::Speed),
    ).unwrap();

    // Resolves model-a -> model-b -> model-c cleanly without duplicate or infinite loop
    assert_eq!(targets.len(), 3);
    assert_eq!(targets[0].physical_model, "model-a");
    assert_eq!(targets[1].physical_model, "model-b");
    assert_eq!(targets[2].physical_model, "model-c");
}

