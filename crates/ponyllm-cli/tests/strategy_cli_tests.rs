use ponyllm_core::pool::{BillingMode, ModelTier};
use ponyllm_cli::config::{ConfigFile, ModelConfig};

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
