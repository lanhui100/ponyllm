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

    // Red-phase expectation: targets should include the primary model and the fallback model
    assert_eq!(targets.len(), 2, "Expected 2 targets (primary + fallback)");
    assert_eq!(targets[0].physical_model, "gemini-3.8-flash-high");
    assert_eq!(targets[1].physical_model, "gemini-3.8-flash-medium");
}
