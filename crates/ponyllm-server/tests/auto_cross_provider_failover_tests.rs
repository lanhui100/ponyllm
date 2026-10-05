use std::sync::Arc;
use std::time::Duration;
use ponyllm_core::pool::{ApiKeyEntry, KeyPool, ModelTier, RoutingStrategy};
use ponyllm_server::{AppState, GatewayConfig, ModelSpec, ProviderConfig};
use ponyllm_server::routes::models::ParsedRequestModel;

#[test]
fn test_auto_cross_provider_failover_when_primary_cooled() {
    let mut config = GatewayConfig::default();
    
    // Provider A: Primary DeepSeek, standard tier
    let p_a = ProviderConfig {
        base_url: "https://api.deepseek.com/v1".to_string(),
        default_model: "deepseek-chat".to_string(),
        models: vec!["deepseek-chat".to_string()],
        model_specs: vec![ModelSpec {
            name: "deepseek-chat".to_string(),
            tier: ModelTier::Standard,
            priority: Some(10),
            ..ModelSpec::default()
        }],
        ..ProviderConfig::default()
    };
    config.providers.insert("provider_a".to_string(), p_a);

    // Provider B: Fallback Qwen, standard tier
    let p_b = ProviderConfig {
        base_url: "https://api.qwen.com/v1".to_string(),
        default_model: "qwen-max".to_string(),
        models: vec!["qwen-max".to_string()],
        model_specs: vec![ModelSpec {
            name: "qwen-max".to_string(),
            tier: ModelTier::Standard,
            priority: Some(5),
            ..ModelSpec::default()
        }],
        ..ProviderConfig::default()
    };
    config.providers.insert("provider_b".to_string(), p_b);

    let state = Arc::new(AppState::new(config));

    // Register pools with keys
    let pool_a = Arc::new(KeyPool::new("provider_a", RoutingStrategy::RoundRobin));
    pool_a.add_key(ApiKeyEntry::new("sk-a1", "sk-a1", 1, 1));
    state.register_pool("provider_a", pool_a.clone());

    let pool_b = Arc::new(KeyPool::new("provider_b", RoutingStrategy::RoundRobin));
    pool_b.add_key(ApiKeyEntry::new("sk-b1", "sk-b1", 1, 1));
    state.register_pool("provider_b", pool_b.clone());

    let parsed = ParsedRequestModel::parse("auto");

    // 1. Initially, both healthy: provider_a wins due to priority
    let targets = state.resolve_routed_targets(&parsed, None).expect("routing should succeed");
    assert!(!targets.is_empty());
    assert_eq!(targets[0].provider_name, "provider_a");

    // 2. Cooldown provider_a (e.g. rate limit 429)
    pool_a.set_key_cooldown("sk-a1", Duration::from_secs(60));

    // After provider_a's keys are cooled, auto routing must automatically deprioritize cooled provider_a
    // and route to healthy provider_b first!
    let targets_after_cd = state.resolve_routed_targets(&parsed, None).expect("routing should succeed");
    assert!(!targets_after_cd.is_empty());
    assert_eq!(
        targets_after_cd[0].provider_name, "provider_b",
        "When provider_a is completely cooled, auto must failover to provider_b"
    );

    // 3. Clear cooldown on provider_a (recovering)
    pool_a.clear_key_cooldown("sk-a1");
    let targets_recovered = state.resolve_routed_targets(&parsed, None).expect("routing should succeed");
    assert_eq!(
        targets_recovered[0].provider_name, "provider_a",
        "When provider_a cooldown expires, it should recover as the primary target"
    );
}
