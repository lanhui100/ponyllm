use clap::Parser;
use ponyllm_cli::config::ConfigFile;
use ponyllm_cli::cli::{Cli, Commands};

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

    let init_cli = Cli::try_parse_from(["ponyllm", "init"]).unwrap();
    match init_cli.command {
        Commands::Init { .. } => {}
        _ => panic!("Expected Init command"),
    }
}
