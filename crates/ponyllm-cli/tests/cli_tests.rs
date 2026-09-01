use clap::Parser;
use ponyllm_cli::config::ConfigFile;
use ponyllm_cli::cli::{Cli, Commands, KeyCommands, ModelCommands, ProviderCommands};

#[test]
fn test_cli_parse_sample_config() {
    let toml_content = r#"
[gateway]
bind = "0.0.0.0:8080"
max_retries = 3
flight_recorder_capacity = 100

[providers.openai]
base_url = "https://api.openai.com"
default_model = "gpt-4o"
strategy = "round_robin"
keys = [
    { id = "k1", api_key = "sk-mock-1", priority = 1, weight = 10 },
    { id = "k2", api_key = "sk-mock-2", priority = 2, weight = 5 },
]

[providers.deepseek]
base_url = "https://api.deepseek.com"
default_model = "deepseek-reasoner"
strategy = "priority"
keys = [
    { id = "ds-1", api_key = "sk-ds-1", priority = 1, weight = 10 },
]
"#;

    let cfg: ConfigFile = toml::from_str(toml_content).unwrap();
    assert_eq!(cfg.gateway.bind, "0.0.0.0:8080");
    assert_eq!(cfg.gateway.max_retries, 3);
    assert_eq!(cfg.providers.len(), 2);
    assert_eq!(cfg.providers["openai"].keys.len(), 2);
    assert_eq!(cfg.providers["deepseek"].default_model, "deepseek-reasoner");
}

#[test]
fn test_cli_commands_parsing() {
    let cli = Cli::try_parse_from(["ponyllm", "serve", "--config", "test.toml", "--bind", "127.0.0.1:9000"]).unwrap();
    match cli.command {
        Commands::Serve { config, bind, .. } => {
            assert_eq!(config, Some("test.toml".to_string()));
            assert_eq!(bind, Some("127.0.0.1:9000".to_string()));
        }
        _ => panic!("Expected Serve command"),
    }

    let init_cli = Cli::try_parse_from(["ponyllm", "init", "--non-interactive"]).unwrap();
    match init_cli.command {
        Commands::Init { non_interactive, .. } => {
            assert!(non_interactive);
        }
        _ => panic!("Expected Init command"),
    }

    let tui_cli = Cli::try_parse_from(["ponyllm", "tui"]).unwrap();
    match tui_cli.command {
        Commands::Tui { gateway_url, .. } => {
            assert_eq!(gateway_url, "http://127.0.0.1:8080");
        }
        _ => panic!("Expected Tui command"),
    }
}

#[test]
fn test_cli_provider_and_key_crud_commands_parsing() {
    // Provider Add
    let p_add = Cli::try_parse_from([
        "ponyllm", "provider", "add", "my-provider",
        "--base-url", "https://api.example.com",
        "--model", "custom-model",
        "--strategy", "priority",
    ]).unwrap();

    match p_add.command {
        Commands::Provider(ProviderCommands::Add { name, base_url, model, strategy, .. }) => {
            assert_eq!(name, "my-provider");
            assert_eq!(base_url, "https://api.example.com");
            assert_eq!(model, "custom-model");
            assert_eq!(strategy, "priority");
        }
        _ => panic!("Expected Provider Add"),
    }

    // Key Add
    let k_add = Cli::try_parse_from([
        "ponyllm", "key", "add",
        "--provider", "my-provider",
        "--id", "key-primary",
        "--key", "sk-secret-123456",
        "--priority", "1",
        "--weight", "20",
    ]).unwrap();

    match k_add.command {
        Commands::Key(KeyCommands::Add { provider, id, key, priority, weight, .. }) => {
            assert_eq!(provider, "my-provider");
            assert_eq!(id, "key-primary");
            assert_eq!(key, "sk-secret-123456");
            assert_eq!(priority, 1);
            assert_eq!(weight, 20);
        }
        _ => panic!("Expected Key Add"),
    }

    // Model Set
    let m_set = Cli::try_parse_from([
        "ponyllm", "model", "set", "deepseek", "deepseek-chat",
    ]).unwrap();

    match m_set.command {
        Commands::Model(ModelCommands::Set { provider, model, .. }) => {
            assert_eq!(provider, "deepseek");
            assert_eq!(model, "deepseek-chat");
        }
        _ => panic!("Expected Model Set"),
    }
}

#[test]
fn test_config_crud_and_mask_methods() {
    let mut cfg = ConfigFile::default();
    assert_eq!(cfg.providers.len(), 0);

    // Add provider
    cfg.add_provider("test-p", "https://api.test.com", "m1", "round_robin");
    assert_eq!(cfg.providers.len(), 1);
    assert_eq!(cfg.providers["test-p"].default_model, "m1");

    // Add key
    cfg.add_key("test-p", "k1", "sk-1234567890abcdef", 1, 10).unwrap();
    assert_eq!(cfg.providers["test-p"].keys.len(), 1);

    // Key masking
    let masked = ConfigFile::mask_key(&cfg.providers["test-p"].keys[0].api_key);
    assert_eq!(masked, "sk-***cdef");

    // Remove key
    let removed = cfg.remove_key("test-p", "k1").unwrap();
    assert!(removed);
    assert_eq!(cfg.providers["test-p"].keys.len(), 0);

    // Remove provider
    let p_removed = cfg.remove_provider("test-p");
    assert!(p_removed);
    assert_eq!(cfg.providers.len(), 0);
}
