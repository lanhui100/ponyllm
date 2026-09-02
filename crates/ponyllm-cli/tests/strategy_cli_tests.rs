use clap::Parser;
use ponyllm_cli::cli::{Cli, Commands, StrategyCommands};
use ponyllm_cli::config::ConfigFile;
use ponyllm_core::pool::GatewayRoutingStrategy;
use std::fs;
use tempfile::NamedTempFile;

#[test]
fn test_strategy_cli_command_parsing() {
    // 1. ponyllm strategy list
    let cli_list = Cli::try_parse_from(["ponyllm", "strategy", "list"]).unwrap();
    match cli_list.command {
        Commands::Strategy(StrategyCommands::List) => {}
        _ => panic!("Expected StrategyCommands::List"),
    }

    // 2. ponyllm strategy get
    let cli_get = Cli::try_parse_from(["ponyllm", "strategy", "get", "-c", "my_config.toml"]).unwrap();
    match cli_get.command {
        Commands::Strategy(StrategyCommands::Get { config }) => {
            assert_eq!(config, Some("my_config.toml".to_string()));
        }
        _ => panic!("Expected StrategyCommands::Get"),
    }

    // 3. ponyllm strategy set speed
    let cli_set = Cli::try_parse_from(["ponyllm", "strategy", "set", "speed", "-c", "my_config.toml"]).unwrap();
    match cli_set.command {
        Commands::Strategy(StrategyCommands::Set { strategy, config }) => {
            assert_eq!(strategy, "speed");
            assert_eq!(config, Some("my_config.toml".to_string()));
        }
        _ => panic!("Expected StrategyCommands::Set"),
    }
}

#[test]
fn test_strategy_config_crud_and_atomic_save() {
    let temp_file = NamedTempFile::new().unwrap();
    let file_path = temp_file.path().to_str().unwrap().to_string();

    let initial_toml = r#"
[gateway]
bind = "127.0.0.1:8080"
default_strategy = "economy"
"#;
    fs::write(&file_path, initial_toml).unwrap();

    let mut cfg = ConfigFile::load_or_default(Some(&file_path)).unwrap();
    assert_eq!(cfg.gateway.default_strategy, GatewayRoutingStrategy::Economy);

    // Switch strategy to Speed and save atomically
    cfg.gateway.default_strategy = GatewayRoutingStrategy::Speed;
    cfg.save_to_path(&file_path).unwrap();

    // Reload and verify persistence
    let reloaded = ConfigFile::load_or_default(Some(&file_path)).unwrap();
    assert_eq!(reloaded.gateway.default_strategy, GatewayRoutingStrategy::Speed);
}
