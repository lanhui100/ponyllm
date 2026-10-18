//! Free-tier suffix transparency in bare-name routing.
//!
//! Users pin a model without a provider prefix (`glm-5.3-flash`). Free
//! upstreams publish that same logical model as `glm-5.3-flash-free`
//! (opencode-zen) or `glm-5.3-flash:free` (OpenRouter). Routing must resolve
//! both spellings to each other while keeping exact same-name matches ahead of
//! the suffix fallback.

use ponyllm_core::pool::BillingMode;
use ponyllm_server::routes::models::ParsedRequestModel;
use ponyllm_server::state::RoutedTarget;
use ponyllm_server::{AppState, GatewayConfig, ProviderConfig};

fn provider(default_model: &str) -> ProviderConfig {
    ProviderConfig {
        egress_pool: vec![],
        egress_strategy: "round_robin".to_string(),
        rate_limits: None,
        base_url: "http://127.0.0.1:9".to_string(),
        default_model: default_model.to_string(),
        strategy: "priority".to_string(),
        billing_mode: BillingMode::Free,
        input_price: 0.0,
        cached_price: 0.0,
        output_price: 0.0,
        models: vec![default_model.to_string()],
        model_specs: Vec::new(),
        default_protocol: None,
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
        timeout_secs: None,
        ttfb_timeout_secs: None,
    }
}

fn state_with(providers: &[(&str, &str)]) -> AppState {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    for (name, model) in providers {
        config
            .providers
            .insert((*name).to_string(), provider(model));
    }
    AppState::new(config)
}

fn resolve(state: &AppState, model: &str) -> Vec<RoutedTarget> {
    let parsed = ParsedRequestModel::parse(model);
    state
        .resolve_routed_targets(&parsed, None)
        .unwrap_or_else(|e| panic!("resolve '{model}' failed: {e}"))
}

fn pairs(targets: &[RoutedTarget]) -> Vec<(String, String)> {
    targets
        .iter()
        .map(|t| (t.provider_name.clone(), t.physical_model.clone()))
        .collect()
}

#[test]
fn bare_base_keeps_exact_match_first_and_appends_free_fallback() {
    let state = state_with(&[("paid", "glm-5.3-flash"), ("zen", "glm-5.3-flash-free")]);

    let targets = resolve(&state, "glm-5.3-flash");

    assert_eq!(
        pairs(&targets),
        vec![
            ("paid".to_string(), "glm-5.3-flash".to_string()),
            ("zen".to_string(), "glm-5.3-flash-free".to_string()),
        ],
        "exact same-name candidate must precede the free-tier variant"
    );
}

#[test]
fn bare_base_resolves_when_only_free_variant_is_configured() {
    let state = state_with(&[("zen", "glm-5.3-flash-free")]);

    let targets = resolve(&state, "glm-5.3-flash");

    assert_eq!(
        pairs(&targets),
        vec![("zen".to_string(), "glm-5.3-flash-free".to_string())]
    );
}

#[test]
fn free_request_falls_back_to_paid_base_when_no_exact_match() {
    let state = state_with(&[("paid", "glm-5.3-flash")]);

    let targets = resolve(&state, "glm-5.3-flash-free");

    assert_eq!(
        pairs(&targets),
        vec![("paid".to_string(), "glm-5.3-flash".to_string())]
    );
}

#[test]
fn colon_and_dash_free_suffixes_are_equivalent() {
    let dash_state = state_with(&[("zen", "glm-5.3-flash-free")]);
    assert_eq!(
        pairs(&resolve(&dash_state, "glm-5.3-flash:free")),
        vec![("zen".to_string(), "glm-5.3-flash-free".to_string())]
    );

    let colon_state = state_with(&[("zen", "glm-5.3-flash:free")]);
    assert_eq!(
        pairs(&resolve(&colon_state, "glm-5.3-flash-free")),
        vec![("zen".to_string(), "glm-5.3-flash:free".to_string())]
    );
}

#[test]
fn exact_free_request_is_never_traded_for_paid_namesake() {
    let state = state_with(&[("paid", "glm-5.3-flash"), ("zen", "glm-5.3-flash-free")]);

    let targets = resolve(&state, "glm-5.3-flash-free");

    assert_eq!(
        pairs(&targets),
        vec![("zen".to_string(), "glm-5.3-flash-free".to_string())],
        "an explicitly configured free model must not fall back to the paid base"
    );
}

#[test]
fn provider_qualified_requests_keep_existing_scope() {
    let state = state_with(&[("paid", "glm-5.3-flash"), ("zen", "glm-5.3-flash-free")]);

    let targets = resolve(&state, "paid/glm-5.3-flash");

    assert_eq!(
        pairs(&targets),
        vec![("paid".to_string(), "glm-5.3-flash".to_string())],
        "provider-qualified requests must not pull in other providers' variants"
    );
}

#[test]
fn non_free_suffix_differences_are_not_prefix_matched() {
    let state = state_with(&[("zen", "glm-5.3-flash-free")]);

    let parsed = ParsedRequestModel::parse("glm-5.3");
    assert!(
        state.resolve_routed_targets(&parsed, None).is_err(),
        "'glm-5.3' must not match 'glm-5.3-flash-free'"
    );
}
