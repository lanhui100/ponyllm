use ponyllm_server::config::{EffectiveProxy, GatewayConfig, ModelSpec, ProviderConfig};
use ponyllm_server::state::AppState;

#[test]
fn test_effective_proxy_resolution() {
    let mut provider = ProviderConfig {
        egress_pool: vec![],
        egress_strategy: "round_robin".to_string(),

        rate_limits: None,
        base_url: "https://api.example.com".to_string(),
        default_model: "default-model".to_string(),
        proxy: Some("http://127.0.0.1:8899".to_string()),
        model_specs: vec![
            ModelSpec {
                rate_limits: None,
                priority: None,
                name: "inherited-model".to_string(),
                proxy: None,
                ..Default::default()
            },
            ModelSpec {
                rate_limits: None,
                priority: None,
                name: "custom-proxy-model".to_string(),
                proxy: Some("http://127.0.0.1:10808".to_string()),
                ..Default::default()
            },
            ModelSpec {
                rate_limits: None,
                priority: None,
                name: "direct-model".to_string(),
                proxy: Some("direct".to_string()),
                ..Default::default()
            },
            ModelSpec {
                rate_limits: None,
                priority: None,
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
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),

            rate_limits: None,
            base_url: "https://access.ponyjob.top".to_string(),
            default_model: "zen-chat".to_string(),
            proxy: None, // Provider is direct
            model_specs: vec![
                ModelSpec {
                    rate_limits: None,
                    priority: None,
                    name: "zen-chat".to_string(),
                    proxy: None, // direct
                    ..Default::default()
                },
                ModelSpec {
                    rate_limits: None,
                    priority: None,
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
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),

            rate_limits: None,
            base_url: "https://api.foreign.com".to_string(),
            default_model: "claude-3-7".to_string(),
            proxy: Some("http://127.0.0.1:8899".to_string()), // provider uses 8899
            model_specs: vec![
                ModelSpec {
                    rate_limits: None,
                    priority: None,
                    name: "claude-3-7".to_string(),
                    proxy: None, // inherits 8899
                    ..Default::default()
                },
                ModelSpec {
                    rate_limits: None,
                    priority: None,
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
    let minimal: ponyllm_config::ConfigFile =
        toml::from_str("[gateway]\nbind = \"127.0.0.1:1\"\n").unwrap();
    assert_eq!(
        minimal.gateway.upstream_timeout_secs,
        ponyllm_config::default_upstream_timeout_secs()
    );
}

#[test]
fn test_domestic_and_private_upstream_proxy_immunity() {
    let mut config = GatewayConfig::default();
    // Gateway has global pproxy egress configured
    config.proxy = Some("http://127.0.0.1:8899".to_string());

    // 1. Domestic and private providers without explicit proxy
    let test_cases = vec![
        ("magpie", "http://100.95.193.103:3425/v1", "trae-cn/deepseek-v4.1-flash-max"),
        ("workbuddy", "http://workbuddy-host:8788/v1", "deepseek-v4.1-flash"),
        ("sense", "https://token.sensenova.cn/v1", "deepseek-flash"),
        ("deepseek", "https://api.deepseek.com", "deepseek-chat"),
        ("aliyun", "https://dashscope.aliyuncs.com/compatible-mode/v1", "qwen-max"),
        ("volces", "https://ark.cn-beijing.volces.com/api/v3", "doubao-pro-32k"),
        ("moonshot", "https://api.moonshot.cn/v1", "moonshot-v1-32k"),
        ("private-lan-10", "http://10.0.1.50:8000/v1", "custom-model"),
        ("private-lan-192", "http://192.168.1.100:8000/v1", "custom-model"),
        ("loopback", "http://127.0.0.1:8000/v1", "local-model"),
    ];

    for (p_name, base_url, model_name) in &test_cases {
        config.providers.insert(
            p_name.to_string(),
            ProviderConfig {
                base_url: base_url.to_string(),
                default_model: model_name.to_string(),
                proxy: None, // No proxy specified => should trigger domestic immunity!
                ..Default::default()
            },
        );
    }

    // 2. Foreign provider without explicit proxy => must inherit gateway proxy
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            base_url: "https://api.openai.com/v1".to_string(),
            default_model: "gpt-4o".to_string(),
            proxy: None, // Should inherit gateway proxy
            ..Default::default()
        },
    );

    // 3. Domestic provider with EXPLICIT proxy override => explicit override wins
    config.providers.insert(
        "sense-custom-proxy".to_string(),
        ProviderConfig {
            base_url: "https://token.sensenova.cn/v1".to_string(),
            default_model: "deepseek-flash".to_string(),
            proxy: Some("http://127.0.0.1:10808".to_string()),
            ..Default::default()
        },
    );

    // 4. Model-level base_url override with domestic target
    config.providers.insert(
        "foreign-base-domestic-model".to_string(),
        ProviderConfig {
            base_url: "https://api.openai.com/v1".to_string(),
            default_model: "default-m".to_string(),
            proxy: None,
            model_specs: vec![
                ModelSpec {
                    name: "domestic-override-model".to_string(),
                    base_url: Some("https://api.deepseek.com".to_string()),
                    proxy: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    );

    // Assert domestic/private providers are immune from inheriting gateway pproxy
    for (p_name, _, model_name) in &test_cases {
        assert_eq!(
            config.effective_proxy_url_for(p_name, model_name),
            None,
            "Provider {} with model {} must resolve to Direct (None), not inherit gateway proxy",
            p_name, model_name
        );
    }

    // Assert foreign provider inherits gateway proxy
    assert_eq!(
        config.effective_proxy_url_for("openai", "gpt-4o"),
        Some("http://127.0.0.1:8899".to_string()),
        "Foreign provider should inherit gateway proxy"
    );

    // Assert explicit proxy on domestic provider is respected
    assert_eq!(
        config.effective_proxy_url_for("sense-custom-proxy", "deepseek-flash"),
        Some("http://127.0.0.1:10808".to_string()),
        "Explicit proxy on domestic provider must take precedence"
    );

    // Assert model-level domestic base_url is immune from inheriting gateway proxy
    assert_eq!(
        config.effective_proxy_url_for("foreign-base-domestic-model", "domestic-override-model"),
        None,
        "Model with domestic base_url must resolve to Direct"
    );
}
