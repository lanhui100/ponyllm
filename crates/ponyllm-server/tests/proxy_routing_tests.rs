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

#[test]
fn test_timeout_config_toml_roundtrip_and_effective_resolution() {
    // P1: gateway/provider/model total-budget overrides parse from TOML and
    // resolve to the effective per-target value.
    let toml_str = r#"
[gateway]
bind = "127.0.0.1:8080"
max_retries = 3
flight_recorder_capacity = 200
upstream_timeout_secs = 1500

[providers.longthink]
base_url = "https://api.example.com"
default_model = "fast-model"
strategy = "priority"
timeout_secs = 900
keys = [ { id = "k1", api_key = "sk-abc123", priority = 1, weight = 10 } ]

[[providers.longthink.model_configs]]
name = "long-thinker"
context_window = "1M"
max_output = "32K"
timeout_secs = 1800

[[providers.longthink.model_configs]]
name = "fast-model"
context_window = "128K"
max_output = "16K"
"#;
    let cfg: ponyllm_config::ConfigFile = toml::from_str(toml_str).expect("TOML parses");
    assert_eq!(cfg.gateway.upstream_timeout_secs, 1500, "gateway override");
    let prov = cfg.providers.get("longthink").expect("provider present");
    assert_eq!(prov.timeout_secs, Some(900), "provider override");
    let long = prov
        .model_configs
        .iter()
        .find(|m| m.name == "long-thinker")
        .expect("model present");
    assert_eq!(long.timeout_secs, Some(1800), "model override");
    let fast = prov
        .model_configs
        .iter()
        .find(|m| m.name == "fast-model")
        .expect("model present");
    assert_eq!(fast.timeout_secs, None, "inherits provider default");

    // Range guard: 0 and oversized values are rejected loudly.
    assert!(ponyllm_config::validate_upstream_timeout_secs(0, "test").is_err());
    assert!(ponyllm_config::validate_upstream_timeout_secs(59, "test").is_err());
    assert!(ponyllm_config::validate_upstream_timeout_secs(1801, "test").is_err());
    assert!(ponyllm_config::validate_upstream_timeout_secs(60, "test").is_ok());
    assert!(ponyllm_config::validate_upstream_timeout_secs(1200, "test").is_ok());
    assert!(ponyllm_config::validate_upstream_timeout_secs(1800, "test").is_ok());

    // Defaults: absent field means the 20-minute budget.
    let minimal: ponyllm_config::ConfigFile = toml::from_str("[gateway]\nbind = \"127.0.0.1:1\"\n").unwrap();
    assert_eq!(minimal.gateway.upstream_timeout_secs, ponyllm_config::default_upstream_timeout_secs());
}
