//! Aggregate (virtual) model routing: declared chain order, cross-provider
//! name mapping, chain-entry cooldown/return, and the paid-fallback cost guard.
//!
//! Contract: `.agents/notes/implemented/architecture/2026-11-23-aggregate-model-priority-chain.md`

use ponyllm_core::pool::{BillingMode, GatewayRoutingStrategy};
use ponyllm_server::routes::models::ParsedRequestModel;
use ponyllm_server::{AppState, GatewayConfig, ModelSpec, ProviderConfig};
use std::time::Duration;

fn provider(
    base_url: &str,
    default_model: &str,
    models: &[&str],
    billing: BillingMode,
) -> ProviderConfig {
    let paid = billing != BillingMode::Free;
    ProviderConfig {
        base_url: base_url.to_string(),
        default_model: default_model.to_string(),
        models: models.iter().map(|m| m.to_string()).collect(),
        model_specs: models
            .iter()
            .map(|m| ModelSpec {
                name: m.to_string(),
                billing_mode: Some(billing),
                input_price: if paid { Some(1.0) } else { Some(0.0) },
                output_price: if paid { Some(1.0) } else { Some(0.0) },
                ..ModelSpec::default()
            })
            .collect(),
        billing_mode: billing,
        input_price: if paid { 1.0 } else { 0.0 },
        output_price: if paid { 1.0 } else { 0.0 },
        ..ProviderConfig::default()
    }
}

fn aggregate(name: &str, chain: &[&str], cooldown_secs: u64) -> ponyllm_config::AggregateModelSpec {
    ponyllm_config::AggregateModelSpec {
        name: name.to_string(),
        chain: chain.iter().map(|e| e.to_string()).collect(),
        cooldown_secs,
    }
}

/// Three providers carrying the same physical model under different names —
/// exactly the sense / magpie / workbuddy shape this feature exists for.
fn three_provider_config() -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.insert(
        "sense".to_string(),
        provider(
            "https://sense.example.com",
            "deepseek-flash",
            &["deepseek-flash"],
            BillingMode::Free,
        ),
    );
    config.providers.insert(
        "magpie".to_string(),
        provider(
            "https://magpie.example.com",
            "deepseek-v4.1-flash",
            &["deepseek-v4.1-flash"],
            BillingMode::Free,
        ),
    );
    config.providers.insert(
        "workbuddy".to_string(),
        provider(
            "https://workbuddy.example.com",
            "deepseek-v4.1-flash",
            &["deepseek-v4.1-flash"],
            BillingMode::Metered,
        ),
    );
    config.aggregate_models = vec![aggregate(
        "deepseek-free",
        &[
            "sense/deepseek-flash",
            "magpie/deepseek-v4.1-flash",
            "workbuddy/deepseek-v4.1-flash",
        ],
        90,
    )];
    config
}

#[tokio::test]
async fn aggregate_chain_order_is_execution_order_across_providers() {
    let state = AppState::new(three_provider_config());
    let targets = state
        .resolve_routed_targets(&ParsedRequestModel::parse("deepseek-free"), None)
        .expect("aggregate resolves");

    let resolved: Vec<(&str, &str)> = targets
        .iter()
        .map(|t| (t.provider_name.as_str(), t.physical_model.as_str()))
        .collect();
    assert_eq!(
        resolved,
        vec![
            ("sense", "deepseek-flash"),
            ("magpie", "deepseek-v4.1-flash"),
            ("workbuddy", "deepseek-v4.1-flash"),
        ],
        "declared chain order must survive untouched (no price/latency re-sort)"
    );
}

#[tokio::test]
async fn aggregate_unknown_entry_is_skipped_not_fatal() {
    let mut config = three_provider_config();
    config.aggregate_models[0].chain.insert(1, "ghost/not-a-model".to_string());
    let state = AppState::new(config);

    let targets = state
        .resolve_routed_targets(&ParsedRequestModel::parse("deepseek-free"), None)
        .expect("one unusable entry must not fail the whole chain");
    assert_eq!(targets.len(), 3);
    assert_eq!(targets[0].provider_name, "sense");
    assert_eq!(targets[1].provider_name, "magpie");
}

#[tokio::test]
async fn aggregate_empty_chain_is_an_error() {
    let mut config = three_provider_config();
    config.aggregate_models[0].chain.clear();
    let state = AppState::new(config);

    let err = state
        .resolve_routed_targets(&ParsedRequestModel::parse("deepseek-free"), None)
        .expect_err("an empty chain cannot route anywhere");
    assert!(
        err.to_string().contains("empty chain"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn aggregate_cooling_entry_is_skipped_then_returns() {
    let state = AppState::new(three_provider_config());

    // Trip the primary's breaker: the next request must start at the secondary.
    state.record_model_outage("sense", "deepseek-flash", Duration::from_millis(80));
    let cooled = state
        .resolve_routed_targets(&ParsedRequestModel::parse("deepseek-free"), None)
        .expect("chain still routes while the primary cools down");
    assert_eq!(
        cooled[0].provider_name, "magpie",
        "a cooling primary must be skipped, not retried first"
    );

    // Once the window expires the primary re-enters at its declared position.
    tokio::time::sleep(Duration::from_millis(120)).await;
    let recovered = state
        .resolve_routed_targets(&ParsedRequestModel::parse("deepseek-free"), None)
        .expect("chain routes after cooldown expiry");
    assert_eq!(
        recovered[0].provider_name, "sense",
        "traffic must return to the primary automatically after the cooldown"
    );
}

#[tokio::test]
async fn aggregate_paid_entry_is_used_as_declared() {
    // 链即授权：运维在链里写了付费项，就按声明顺序使用，不再有额外开关。
    let state = AppState::new(three_provider_config());
    let targets = state
        .resolve_routed_targets(&ParsedRequestModel::parse("deepseek-free"), None)
        .expect("all declared chain entries resolve");
    let providers: Vec<&str> = targets.iter().map(|t| t.provider_name.as_str()).collect();
    assert_eq!(
        providers,
        vec!["sense", "magpie", "workbuddy"],
        "paid entries declared in the chain must be tried in declared order"
    );
}

#[tokio::test]
async fn physical_model_name_shadows_aggregate_name() {
    let mut config = three_provider_config();
    // A provider that really serves a model called `deepseek-free` wins.
    config.providers.insert(
        "shadow".to_string(),
        provider(
            "https://shadow.example.com",
            "deepseek-free",
            &["deepseek-free"],
            BillingMode::Free,
        ),
    );
    let state = AppState::new(config);

    let targets = state
        .resolve_routed_targets(&ParsedRequestModel::parse("deepseek-free"), None)
        .expect("physical name resolves");
    assert!(
        targets.iter().all(|t| t.provider_name == "shadow"),
        "a physical model name must shadow an aggregate of the same name"
    );
}

#[tokio::test]
async fn aggregate_is_listed_for_client_discovery() {
    let state = AppState::new(three_provider_config());
    let ids: Vec<String> = state
        .list_all_models_with_capacity()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert!(
        ids.iter().any(|id| id == "deepseek-free"),
        "aggregate models must be discoverable via the model list; got {ids:?}"
    );
    // The listing must not have dropped the physical entries.
    assert!(ids.iter().any(|id| id == "sense/deepseek-flash"));
}

#[tokio::test]
async fn aggregate_cooldown_reads_the_configured_window() {
    let state = AppState::new(three_provider_config());
    assert_eq!(
        state.aggregate_cooldown("deepseek-free"),
        Some(Duration::from_secs(90))
    );
    assert_eq!(
        state.aggregate_cooldown("sense/deepseek-flash"),
        None,
        "a physical model keeps the legacy 600s auto breaker window"
    );
}

#[tokio::test]
async fn aggregate_chain_survives_strategy_headers() {
    // A client-supplied strategy must not reshuffle a declared chain.
    let state = AppState::new(three_provider_config());
    let targets = state
        .resolve_routed_targets(
            &ParsedRequestModel::parse("deepseek-free"),
            Some(GatewayRoutingStrategy::Economy),
        )
        .expect("aggregate resolves");
    assert_eq!(targets[0].provider_name, "sense");
    assert_eq!(targets[2].provider_name, "workbuddy");
}
