//! Adversarial acceptance tests for wave-8 model surface contract
//! Contract: .dev-team/contracts/wave-8-model-surface.md

use ponyllm_core::pool::BillingMode;
use ponyllm_server::config::{ModelSpec, ProviderConfig};
use ponyllm_server::routes::models::ParsedRequestModel;
use ponyllm_server::{AppState, GatewayConfig};

fn make_adversarial_config() -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;

    let p1 = ProviderConfig {
        base_url: "https://p1.example.com/v1".to_string(),
        default_model: "test-model".to_string(),
        strategy: "priority".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 1.0,
        cached_price: 0.5,
        output_price: 2.0,
        models: vec!["a/b/c".to_string()],
        model_specs: vec![
            ModelSpec {
                name: "test-model".to_string(),
                priority: Some(10),
                context_window: "1M".to_string(),
                ..ModelSpec::default()
            },
            ModelSpec {
                name: "a/b/c".to_string(),
                priority: Some(10),
                context_window: "1M".to_string(),
                ..ModelSpec::default()
            },
        ],
        ..ProviderConfig::default()
    };

    config.providers.insert("prov1".to_string(), p1);
    config
}

#[test]
fn adv_no_1m_tag_in_ids_even_with_1m_context_window() {
    let state = AppState::new(make_adversarial_config());
    let models = state.list_all_models();
    for (id, _, _, _) in models {
        assert!(!id.contains("[1m]"), "Found [1m] in listed model ID: {id}");
        assert!(!id.contains("[1024k]"), "Found [1024k] in listed model ID: {id}");
    }
}

#[test]
fn adv_multi_segment_model_name_listed_with_provider() {
    let state = AppState::new(make_adversarial_config());
    let models = state.list_all_models();
    let ids: Vec<&str> = models.iter().map(|(id, _, _, _)| id.as_str()).collect();

    assert!(ids.contains(&"prov1/a/b/c"), "Must contain multi-segment id prov1/a/b/c");
    assert!(!ids.contains(&"a/b/c"), "Must NOT contain bare multi-segment id a/b/c");
}
