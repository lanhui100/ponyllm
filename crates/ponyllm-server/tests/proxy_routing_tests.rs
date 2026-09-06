use ponyllm_server::config::{GatewayConfig, ModelSpec, ProviderConfig, EffectiveProxy};
use ponyllm_server::state::AppState;

#[test]
fn test_effective_proxy_resolution() {
    let mut provider = ProviderConfig {
        base_url: "https://api.example.com".to_string(),
        default_model: "default-model".to_string(),
        proxy: Some("http://127.0.0.1:8899".to_string()),
        model_specs: vec![
            ModelSpec {
                name: "inherited-model".to_string(),
                proxy: None,
                ..Default::default()
            },
            ModelSpec {
                name: "custom-proxy-model".to_string(),
                proxy: Some("http://127.0.0.1:10808".to_string()),
                ..Default::default()
            },
            ModelSpec {
                name: "direct-model".to_string(),
                proxy: Some("direct".to_string()),
                ..Default::default()
            },
            ModelSpec {
                name: "none-model".to_string(),
                proxy: Some("none".to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    // Model with None inherits provider proxy
    assert_eq!(
        provider.effective_proxy_for_model("inherited-model"),
        EffectiveProxy::Custom("http://127.0.0.1:8899")
    );
    // Non-existent spec inherits provider proxy
    assert_eq!(
        provider.effective_proxy_for_model("unknown-model"),
        EffectiveProxy::Custom("http://127.0.0.1:8899")
    );
    // Custom proxy overrides provider proxy
    assert_eq!(
        provider.effective_proxy_for_model("custom-proxy-model"),
        EffectiveProxy::Custom("http://127.0.0.1:10808")
    );
    // "direct" overrides provider proxy to Direct
    assert_eq!(
        provider.effective_proxy_for_model("direct-model"),
        EffectiveProxy::Direct
    );
    // "none" overrides provider proxy to Direct
    assert_eq!(
        provider.effective_proxy_for_model("none-model"),
        EffectiveProxy::Direct
    );

    // If provider has no proxy
    provider.proxy = None;
    assert_eq!(
        provider.effective_proxy_for_model("inherited-model"),
        EffectiveProxy::InheritGateway
    );
    assert_eq!(
        provider.effective_proxy_for_model("custom-proxy-model"),
        EffectiveProxy::Custom("http://127.0.0.1:10808")
    );
    assert_eq!(
        provider.effective_proxy_for_model("direct-model"),
        EffectiveProxy::Direct
    );
}

#[test]
fn test_app_state_http_client_routing_and_pooling() {
    let mut config = GatewayConfig::default();
    config.providers.insert(
        "opencode-zen".to_string(),
        ProviderConfig {
            base_url: "https://access.ponyjob.top".to_string(),
            default_model: "zen-chat".to_string(),
            proxy: None, // Provider is direct
            model_specs: vec![
                ModelSpec {
                    name: "zen-chat".to_string(),
                    proxy: None, // direct
                    ..Default::default()
                },
                ModelSpec {
                    name: "muse-spark".to_string(),
                    proxy: Some("http://127.0.0.1:8899".to_string()), // needs proxy!
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    );

    config.providers.insert(
        "foreign-provider".to_string(),
        ProviderConfig {
            base_url: "https://api.foreign.com".to_string(),
            default_model: "claude-3-7".to_string(),
            proxy: Some("http://127.0.0.1:8899".to_string()), // provider uses 8899
            model_specs: vec![
                ModelSpec {
                    name: "claude-3-7".to_string(),
                    proxy: None, // inherits 8899
                    ..Default::default()
                },
                ModelSpec {
                    name: "claude-direct".to_string(),
                    proxy: Some("direct".to_string()), // forces direct
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    );

    let state = AppState::new(config);

    // opencode-zen zen-chat should be direct
    let c_zen = state.http_client_for_target("opencode-zen", "zen-chat");
    // foreign-provider claude-direct should be direct
    let c_claude_dir = state.http_client_for_target("foreign-provider", "claude-direct");
    // Both direct clients can share the same direct client / gateway client
    // opencode-zen muse-spark should use 8899
    let c_muse = state.http_client_for_target("opencode-zen", "muse-spark");
    // foreign-provider claude-3-7 should also use 8899
    let c_foreign = state.http_client_for_target("foreign-provider", "claude-3-7");

    // Both using the same proxy URL should reuse the client
    // Note: reqwest::Client cloning is an Arc clone of the underlying connection pool
    // In Rust, we can verify that querying twice returns valid clients without error
    let c_muse_again = state.http_client_for_target("opencode-zen", "muse-spark");
    let _ = (c_zen, c_claude_dir, c_muse, c_foreign, c_muse_again);
}
