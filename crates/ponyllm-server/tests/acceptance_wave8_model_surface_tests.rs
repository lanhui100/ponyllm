//! Acceptance tests for wave-8 model surface contract
//! Contract: .dev-team/contracts/wave-8-model-surface.md

use ponyllm_core::pool::BillingMode;
use ponyllm_server::config::{ModelSpec, ProviderConfig};
use ponyllm_server::routes::models::ParsedRequestModel;
use ponyllm_server::{AppState, GatewayConfig};

fn make_test_config() -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;

    // Provider alpha: shared-model (1M, priority 10), solo-model (128K, priority 5)
    let mut alpha_cfg = ProviderConfig {
        base_url: "https://alpha.example.com/v1".to_string(),
        default_model: "shared-model".to_string(),
        strategy: "priority".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 1.0,
        cached_price: 0.5,
        output_price: 2.0,
        models: vec!["solo-model".to_string()],
        model_specs: vec![
            ModelSpec {
                name: "shared-model".to_string(),
                priority: Some(10),
                context_window: "1M".to_string(),
                ..ModelSpec::default()
            },
            ModelSpec {
                name: "solo-model".to_string(),
                priority: Some(5),
                context_window: "128K".to_string(),
                ..ModelSpec::default()
            },
        ],
        ..ProviderConfig::default()
    };

    // Provider beta: shared-model (1M, priority 1)
    let mut beta_cfg = ProviderConfig {
        base_url: "https://beta.example.com/v1".to_string(),
        default_model: "shared-model".to_string(),
        strategy: "priority".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 1.0,
        cached_price: 0.5,
        output_price: 2.0,
        models: vec![],
        model_specs: vec![ModelSpec {
            name: "shared-model".to_string(),
            priority: Some(1),
            context_window: "1M".to_string(),
            ..ModelSpec::default()
        }],
        ..ProviderConfig::default()
    };

    // Provider zen: multi-segment model "convaiinnovations/laya"
    let mut zen_cfg = ProviderConfig {
        base_url: "https://zen.example.com/v1".to_string(),
        default_model: "convaiinnovations/laya".to_string(),
        strategy: "priority".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 1.0,
        cached_price: 0.5,
        output_price: 2.0,
        models: vec![],
        model_specs: vec![ModelSpec {
            name: "convaiinnovations/laya".to_string(),
            priority: Some(2),
            context_window: "200K".to_string(),
            ..ModelSpec::default()
        }],
        ..ProviderConfig::default()
    };

    config.providers.insert("alpha".to_string(), alpha_cfg);
    config.providers.insert("beta".to_string(), beta_cfg);
    config.providers.insert("zen".to_string(), zen_cfg);

    config
}

#[test]
fn c1_01_no_1m_in_list_all_models() {
    let state = AppState::new(make_test_config());
    let models = state.list_all_models();
    for (id, _, _, _) in &models {
        assert!(
            !id.to_lowercase().contains("[1m]") && !id.to_lowercase().contains("[1024k]"),
            "id '{id}' contains [1m] or [1024k]"
        );
    }
}

#[test]
fn c2_01_and_c2_02_all_physical_models_prefixed_and_no_bare_models() {
    let state = AppState::new(make_test_config());
    let models = state.list_all_models();
    let ids: Vec<&str> = models.iter().map(|(id, _, _, _)| id.as_str()).collect();

    assert!(ids.contains(&"auto"), "auto must be present");
    assert!(ids.contains(&"alpha/shared-model"));
    assert!(ids.contains(&"alpha/solo-model"));
    assert!(ids.contains(&"beta/shared-model"));
    assert!(ids.contains(&"zen/convaiinnovations/laya"));

    // Bare physical models must not be in the list
    assert!(!ids.contains(&"shared-model"), "bare shared-model must not be listed");
    assert!(!ids.contains(&"solo-model"), "bare solo-model must not be listed");
    assert!(!ids.contains(&"convaiinnovations/laya"), "bare multi-segment name must not be listed");
}

#[test]
fn c3_01_auto_is_first_and_ponyllm_owned() {
    let state = AppState::new(make_test_config());
    let models = state.list_all_models();
    assert!(!models.is_empty());
    assert_eq!(models[0].0, "auto");
    assert_eq!(models[0].1, "ponyllm");
    assert_eq!(models[0].3, "auto");
}

#[test]
fn c4_01_c4_02_deterministic_provider_lexicographical_order() {
    let state = AppState::new(make_test_config());
    let models = state.list_all_models();
    let ids: Vec<&str> = models.iter().map(|(id, _, _, _)| id.as_str()).collect();

    // After auto, entries should be alpha block, then beta block, then zen block
    let alpha_pos = ids.iter().position(|id| id.starts_with("alpha/")).unwrap();
    let beta_pos = ids.iter().position(|id| id.starts_with("beta/")).unwrap();
    let zen_pos = ids.iter().position(|id| id.starts_with("zen/")).unwrap();

    assert!(alpha_pos < beta_pos);
    assert!(beta_pos < zen_pos);
}

#[test]
fn c7_02_bare_name_priority_ordering() {
    let state = AppState::new(make_test_config());
    let parsed = ParsedRequestModel::parse("shared-model");
    let targets = state.resolve_routed_targets(&parsed, None).expect("resolved targets");

    assert!(targets.len() >= 2, "shared-model should resolve to at least 2 candidates");
    assert_eq!(
        targets[0].provider_name, "alpha",
        "alpha has priority 10 vs beta priority 1, must rank first"
    );
    assert_eq!(targets[1].provider_name, "beta");
}
