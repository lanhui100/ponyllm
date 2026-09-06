use ponyllm_cli::config::{ConfigFile, ModelConfig};
use tempfile::NamedTempFile;

#[test]
fn test_config_file_model_proxy_serialization() {
    let toml_str = r#"
[gateway]
bind = "127.0.0.1:8080"
proxy = "http://127.0.0.1:7890"

[providers.opencode-zen]
base_url = "https://access.ponyjob.top"
default_model = "zen-chat"
strategy = "round_robin"
proxy = "http://127.0.0.1:8899"

[[providers.opencode-zen.model_configs]]
name = "zen-chat"
# proxy omitted -> inherits http://127.0.0.1:8899

[[providers.opencode-zen.model_configs]]
name = "zen-fast"
proxy = "direct"

[[providers.opencode-zen.model_configs]]
name = "muse-spark"
proxy = "http://127.0.0.1:10808"
"#;

    let cfg: ConfigFile = toml::from_str(toml_str).expect("Failed to deserialize TOML");
    let provider = cfg.providers.get("opencode-zen").expect("opencode-zen missing");
    assert_eq!(provider.proxy.as_deref(), Some("http://127.0.0.1:8899"));

    let m_zen = provider.get_model_config("zen-chat");
    assert_eq!(m_zen.proxy, None);

    let m_fast = provider.get_model_config("zen-fast");
    assert_eq!(m_fast.proxy.as_deref(), Some("direct"));

    let m_muse = provider.get_model_config("muse-spark");
    assert_eq!(m_muse.proxy.as_deref(), Some("http://127.0.0.1:10808"));

    // Verify round-trip serialization
    let serialized = toml::to_string_pretty(&cfg).expect("Failed to serialize");
    let roundtrip: ConfigFile = toml::from_str(&serialized).expect("Failed to parse roundtrip");
    let rt_p = roundtrip.providers.get("opencode-zen").unwrap();
    assert_eq!(rt_p.get_model_config("zen-fast").proxy.as_deref(), Some("direct"));
    assert_eq!(rt_p.get_model_config("muse-spark").proxy.as_deref(), Some("http://127.0.0.1:10808"));
}

#[test]
fn test_upsert_model_config_with_proxy() {
    let mut cfg = ConfigFile::default();
    cfg.add_provider("test-p", "https://api.example.com", "default-model", "round_robin");
    let provider = cfg.providers.get_mut("test-p").unwrap();

    let mut model_cfg = ModelConfig::new("test-model");
    model_cfg.proxy = Some("http://127.0.0.1:8899".to_string());
    provider.upsert_model_config(model_cfg);

    let fetched = provider.get_model_config("test-model");
    assert_eq!(fetched.proxy.as_deref(), Some("http://127.0.0.1:8899"));

    // Update to direct
    let mut update_cfg = ModelConfig::new("test-model");
    update_cfg.proxy = Some("direct".to_string());
    provider.upsert_model_config(update_cfg);

    let fetched_again = provider.get_model_config("test-model");
    assert_eq!(fetched_again.proxy.as_deref(), Some("direct"));
}

#[test]
fn test_config_atomic_save_and_reload() {
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap();

    let mut cfg = ConfigFile::default();
    cfg.add_provider("test-p", "https://api.test.com", "m1", "round_robin");
    if let Some(p) = cfg.providers.get_mut("test-p") {
        p.proxy = Some("http://127.0.0.1:8899".to_string());
        p.upsert_model_config(ModelConfig {
            name: "m1".to_string(),
            proxy: Some("direct".to_string()),
            ..Default::default()
        });
    }

    cfg.save_to_path(path).unwrap();

    let loaded = ConfigFile::load_or_default(Some(path)).unwrap();
    let p = loaded.providers.get("test-p").unwrap();
    assert_eq!(p.proxy.as_deref(), Some("http://127.0.0.1:8899"));
    assert_eq!(p.get_model_config("m1").proxy.as_deref(), Some("direct"));
}
