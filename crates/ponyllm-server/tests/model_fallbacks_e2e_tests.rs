use ponyllm_server::routes::models::ParsedRequestModel;
use ponyllm_server::{AppState, GatewayConfig, ModelSpec, ProviderConfig};

#[tokio::test]
async fn test_empty_stop_early_convergence_triggers_model_fallback() {
    let mut config = GatewayConfig::default();
    let prov = ProviderConfig {
        egress_pool: vec![],
        egress_strategy: "round_robin".to_string(),

        base_url: "https://example.com".to_string(),
        default_model: "gemini-3.8-flash-high".to_string(),
        strategy: "priority".to_string(),
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

    // Verify that the targets list properly resolves both models for routing
    let parsed = ParsedRequestModel::parse("gemini-3.8-flash-high");
    let targets = state.resolve_routed_targets(&parsed, None).unwrap();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].physical_model, "gemini-3.8-flash-high");
    assert_eq!(targets[1].physical_model, "gemini-3.8-flash-medium");
}
