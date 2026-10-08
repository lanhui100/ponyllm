use ponyllm_server::{AppState, GatewayConfig, ProviderConfig};
use ponyllm_server::routes::models::ParsedRequestModel;
use ponyllm_core::pool::BillingMode;

#[tokio::test]
async fn test_pure_auto_ranking_and_failover_circuit_breaker() {
    let mut config = GatewayConfig::default();
    config.auto_models = vec![
        "gemini-3.8-flash".to_string(),
        "deepseek-v4-flash".to_string(),
    ];

    // Provider A: Paid provider carrying deepseek-v4-flash
    let mut p_paid = ProviderConfig::default();
    p_paid.base_url = "https://paid.example.com".to_string();
    p_paid.default_model = "deepseek-v4-flash".to_string();
    p_paid.billing_mode = BillingMode::Metered;
    p_paid.models = vec!["deepseek-v4-flash".to_string()];
    config.providers.insert("paid_provider".to_string(), p_paid);

    // Provider B: Free provider carrying gemini-3.8-flash (Free billing mode)
    let mut p_free = ProviderConfig::default();
    p_free.base_url = "https://free.example.com".to_string();
    p_free.default_model = "gemini-3.8-flash".to_string();
    p_free.billing_mode = BillingMode::Free;
    p_free.models = vec!["gemini-3.8-flash".to_string()];
    config.providers.insert("free_provider".to_string(), p_free);

    let state = AppState::new(config);

    // 1. Initial resolution: free provider gemini-3.8-flash MUST rank first!
    let parsed = ParsedRequestModel::parse("auto");
    let targets = state.resolve_routed_targets(&parsed, None).expect("Must resolve auto targets");
    assert!(!targets.is_empty());
    assert_eq!(targets[0].provider_name, "free_provider");
    assert_eq!(targets[0].physical_model, "gemini-3.8-flash");
    assert_eq!(targets[1].provider_name, "paid_provider");
    assert_eq!(targets[1].physical_model, "deepseek-v4-flash");

    // 2. Simulate sudden outage on free gemini-3.8-flash (Model Circuit Breaker tripped)
    state.record_model_outage("free_provider", "gemini-3.8-flash", std::time::Duration::from_secs(600));
    assert!(state.is_model_cooling_down("free_provider", "gemini-3.8-flash"));

    // 3. Subsequent resolution: cooled down model is deprioritized, paid deepseek-v4-flash wins failover!
    let targets_after_outage = state.resolve_routed_targets(&parsed, None).expect("Must resolve auto targets");
    assert_eq!(targets_after_outage[0].provider_name, "paid_provider");
    assert_eq!(targets_after_outage[0].physical_model, "deepseek-v4-flash");
    assert_eq!(targets_after_outage[1].provider_name, "free_provider");
}
