use ponyllm_core::pool::{BillingMode, ModelTier};
use ponyllm_cli::config::{validate_provider_fields, ConfigFile, ModelConfig};

#[test]
fn test_model_config_tier_and_pricing_serialization() {
    let mut cfg = ConfigFile::default();
    cfg.add_provider_full(
        "deepseek",
        "https://api.deepseek.com",
        "deepseek-chat",
        "round_robin",
        BillingMode::Metered,
        0.14,
        0.014,
        0.28,
    );

    let m_custom = ModelConfig {
        name: "deepseek-chat".to_string(),
        tier: ModelTier::Standard,
        context_window: "128K".to_string(),
        max_output: "8K".to_string(),
        input_types: vec!["text".to_string()],
        output_types: vec!["text".to_string()],
        billing_mode: Some(BillingMode::Plan),
        input_price: Some(0.10),
        cached_price: Some(0.01),
        output_price: Some(0.20),
        protocol: None,
        thinking_default: None,
        thinking_max: None,
    };

    let m_inherit = ModelConfig {
        name: "deepseek-coder".to_string(),
        tier: ModelTier::Flagship,
        context_window: "64K".to_string(),
        max_output: "4K".to_string(),
        input_types: vec!["text".to_string()],
        output_types: vec!["text".to_string()],
        billing_mode: None,
        input_price: None,
        cached_price: None,
        output_price: None,
        protocol: None,
        thinking_default: None,
        thinking_max: None,
    };


    cfg.upsert_model_config("deepseek", m_custom).unwrap();
    cfg.upsert_model_config("deepseek", m_inherit).unwrap();

    let p = &cfg.providers["deepseek"];
    let pr_custom = p.get_model_pricing("deepseek-chat");
    assert_eq!(pr_custom.input_price, 0.10);
    assert_eq!(pr_custom.cached_price, 0.01);
    assert_eq!(pr_custom.output_price, 0.20);
    assert_eq!(p.get_model_billing_mode("deepseek-chat"), BillingMode::Plan);

    let pr_inherit = p.get_model_pricing("deepseek-coder");
    assert_eq!(pr_inherit.input_price, 0.14);
    assert_eq!(pr_inherit.cached_price, 0.014);
    assert_eq!(pr_inherit.output_price, 0.28);
    assert_eq!(p.get_model_billing_mode("deepseek-coder"), BillingMode::Metered);

    // Verify TOML roundtrip
    let toml_str = toml::to_string(&cfg).unwrap();
    let reloaded: ConfigFile = toml::from_str(&toml_str).unwrap();
    let reloaded_p = &reloaded.providers["deepseek"];
    assert_eq!(reloaded_p.get_model_pricing("deepseek-chat").input_price, 0.10);
    assert_eq!(reloaded_p.get_model_pricing("deepseek-coder").input_price, 0.14);
    assert_eq!(reloaded_p.get_model_billing_mode("deepseek-chat"), BillingMode::Plan);
    assert_eq!(reloaded_p.get_model_billing_mode("deepseek-coder"), BillingMode::Metered);
    assert_eq!(reloaded_p.get_model_config("deepseek-chat").tier, ModelTier::Standard);
    assert_eq!(reloaded_p.get_model_config("deepseek-coder").tier, ModelTier::Flagship);
}

#[test]
fn test_old_config_without_protocol_fields_loads_with_heuristic_fallback() {
    let old_toml = r#"
[gateway]
bind = "127.0.0.1:8080"
max_retries = 3
flight_recorder_capacity = 200
api_key = "test-key"

[providers.deepseek]
base_url = "https://api.deepseek.com"
default_model = "deepseek-chat"
strategy = "priority"
keys = [
    { id = "k1", api_key = "sk-x", priority = 1, weight = 10 },
]
"#;
    let cfg: ConfigFile = toml::from_str(old_toml).unwrap();
    let p = &cfg.providers["deepseek"];
    assert_eq!(p.default_protocol, None);
    assert_eq!(p.chat_url, None);
    assert_eq!(p.responses_url, None);
    assert_eq!(p.messages_url, None);
    assert_eq!(p.get_model_config("deepseek-chat").protocol, None);
}

#[test]
fn test_protocol_fields_toml_roundtrip() {
    use ponyllm_core::pool::UpstreamProtocol;
    let mut cfg = ConfigFile::default();
    cfg.add_provider_full(
        "op",
        "https://op.example.com",
        "muse-spark",
        "round_robin",
        BillingMode::Metered,
        0.1,
        0.01,
        0.2,
    );
    let p = cfg.providers.get_mut("op").unwrap();
    p.default_protocol = Some(UpstreamProtocol::Responses);
    p.responses_url = Some("https://resp.example.com/v1".to_string());
    let toml_str = toml::to_string(&cfg).unwrap();
    assert!(toml_str.contains("default_protocol"));
    let reloaded: ConfigFile = toml::from_str(&toml_str).unwrap();
    let rp = &reloaded.providers["op"];
    assert_eq!(rp.default_protocol, Some(UpstreamProtocol::Responses));
    assert_eq!(rp.responses_url.as_deref(), Some("https://resp.example.com/v1"));
}

#[test]
fn test_validate_provider_fields_rejects_garbage() {
    assert!(validate_provider_fields(
        "https://api.example.com",
        "m",
        "priority",
        "metered",
        None,
        None,
        None
    )
    .is_ok());
    assert!(validate_provider_fields("ftp://x", "m", "priority", "metered", None, None, None).is_err());
    assert!(validate_provider_fields("https://x", "  ", "priority", "metered", None, None, None).is_err());
    assert!(validate_provider_fields("https://x", "m", "random", "metered", None, None, None).is_err());
    assert!(validate_provider_fields("https://x", "m", "priority", "gold", None, None, None).is_err());
    assert!(validate_provider_fields(
        "https://x",
        "m",
        "priority",
        "metered",
        Some("notaurl"),
        None,
        None
    )
    .is_err());
}
