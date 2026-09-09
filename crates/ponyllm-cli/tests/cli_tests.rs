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
default_model = "deepseek-v4-flash"
strategy = "priority"
keys = [
    { id = "ds-1", api_key = "sk-ds-1", priority = 1, weight = 10 },
]

[providers.deepseek-anthropic]
base_url = "https://api.deepseek.com/anthropic"
default_model = "deepseek-v4-flash"
strategy = "priority"
keys = [
    { id = "ds-ant-1", api_key = "sk-ds-1", priority = 1, weight = 10 },
]
"#;

    let cfg: ConfigFile = toml::from_str(toml_content).unwrap();
    assert_eq!(cfg.gateway.bind, "0.0.0.0:8080");
    assert_eq!(cfg.gateway.max_retries, 3);
    assert_eq!(cfg.providers.len(), 3);
    assert_eq!(cfg.providers["openai"].keys.len(), 2);
    assert_eq!(cfg.providers["deepseek"].default_model, "deepseek-v4-flash");
    assert_eq!(cfg.providers["deepseek-anthropic"].base_url, "https://api.deepseek.com/anthropic");
}

#[test]
fn test_cli_commands_parsing() {
    let cli = Cli::try_parse_from(["ponyllm", "serve", "--config", "test.toml", "-a", "0.0.0.0", "-p", "9000", "--api-key", "sk-test-tok", "--debug"]).unwrap();
    match cli.command {
        Commands::Serve { config, address, port, api_key, debug, .. } => {
            assert_eq!(config, Some("test.toml".to_string()));
            assert_eq!(address, Some("0.0.0.0".to_string()));
            assert_eq!(port, Some(9000));
            assert_eq!(api_key, Some("sk-test-tok".to_string()));
            assert!(debug);
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

    let stop_cli = Cli::try_parse_from(["ponyllm", "stop", "--config", "test.toml"]).unwrap();
    match stop_cli.command {
        Commands::Stop { config } => {
            assert_eq!(config, Some("test.toml".to_string()));
        }
        _ => panic!("Expected Stop command"),
    }

    let restart_cli = Cli::try_parse_from(["ponyllm", "restart", "-p", "9000"]).unwrap();
    match restart_cli.command {
        Commands::Restart { port, config, .. } => {
            assert_eq!(port, Some(9000));
            assert_eq!(config, None);
        }
        _ => panic!("Expected Restart command"),
    }

    let web_cli = Cli::try_parse_from(["ponyllm", "web"]).unwrap();
    match web_cli.command {
        Commands::Web { port, address, no_open, .. } => {
            assert_eq!(port, 18080);
            assert_eq!(address, "127.0.0.1");
            assert!(!no_open);
        }
        _ => panic!("Expected Web command"),
    }

    let web_custom = Cli::try_parse_from(["ponyllm", "web", "-p", "18099", "--open"]).unwrap();
    match web_custom.command {
        Commands::Web { port, open, no_open, .. } => {
            assert_eq!(port, 18099);
            assert!(open);
            assert!(!no_open);
        }
        _ => panic!("Expected Web command"),
    }

    let web_no_open = Cli::try_parse_from(["ponyllm", "web", "--no-open"]).unwrap();
    match web_no_open.command {
        Commands::Web { no_open, .. } => {
            assert!(no_open);
        }
        _ => panic!("Expected Web command"),
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

    // Model Add
    let m_add = Cli::try_parse_from([
        "ponyllm", "model", "add", "deepseek", "deepseek-reasoner",
    ]).unwrap();

    match m_add.command {
        Commands::Model(ModelCommands::Add { provider, model, context, max_output, .. }) => {
            assert_eq!(provider, "deepseek");
            assert_eq!(model, "deepseek-reasoner");
            assert_eq!(context, "1M");
            assert_eq!(max_output, "32K");
        }
        _ => panic!("Expected Model Add"),
    }

    // Model Add with parameters
    let m_add_params = Cli::try_parse_from([
        "ponyllm", "model", "add", "openai", "gpt-4o",
        "--context", "2M",
        "--max-output", "64K",
        "--inputs", "text,image",
        "--outputs", "text",
    ]).unwrap();

    match m_add_params.command {
        Commands::Model(ModelCommands::Add { provider, model, context, max_output, inputs, outputs, .. }) => {
            assert_eq!(provider, "openai");
            assert_eq!(model, "gpt-4o");
            assert_eq!(context, "2M");
            assert_eq!(max_output, "64K");
            assert_eq!(inputs, "text,image");
            assert_eq!(outputs, "text");
        }
        _ => panic!("Expected Model Add with parameters"),
    }

    // Model Remove
    let m_remove = Cli::try_parse_from([
        "ponyllm", "model", "remove", "deepseek", "deepseek-reasoner",
    ]).unwrap();

    match m_remove.command {
        Commands::Model(ModelCommands::Remove { provider, model, .. }) => {
            assert_eq!(provider, "deepseek");
            assert_eq!(model, "deepseek-reasoner");
        }
        _ => panic!("Expected Model Remove"),
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

    // Add additional model
    cfg.add_model("test-p", "m2").unwrap();
    cfg.add_model("test-p", "m3").unwrap();
    assert_eq!(cfg.providers["test-p"].models, vec!["m2", "m3"]);

    // Remove additional model
    let m_removed = cfg.remove_model("test-p", "m2").unwrap();
    assert!(m_removed);
    // Set default model (old default 'm1' atomically migrates to models)
    cfg.set_default_model("test-p", "m-new").unwrap();
    assert_eq!(cfg.providers["test-p"].default_model, "m-new");
    assert_eq!(cfg.providers["test-p"].models, vec!["m3", "m1"]);

    // Attempting to remove active default model should be rejected
    let del_default_err = cfg.remove_model("test-p", "m-new");
    assert!(del_default_err.is_err());
    assert!(del_default_err.unwrap_err().contains("无法直接删除默认主模型"));

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

#[test]
fn test_model_config_crud_and_params() {
    use ponyllm_cli::config::ModelConfig;

    let mut cfg = ConfigFile::default();
    cfg.add_provider("ai-hub", "https://api.aihub.com", "default-chat", "priority");

    // 1. Check default model params automatically populated with defaults
    let p = &cfg.providers["ai-hub"];
    let def_cfg = p.get_model_config("default-chat");
    assert_eq!(def_cfg.name, "default-chat");
    assert_eq!(def_cfg.context_window, "1M");
    assert_eq!(def_cfg.max_output, "32K");
    assert_eq!(def_cfg.input_types, vec!["text".to_string()]);
    assert_eq!(def_cfg.output_types, vec!["text".to_string()]);

    // 2. Add custom model with multimodal parameters
    let custom_model = ModelConfig {
        name: "omni-v1".to_string(),
        tier: ponyllm_core::pool::ModelTier::Flagship,
        context_window: "2M".to_string(),
        max_output: "64K".to_string(),
        input_types: vec!["text".to_string(), "image".to_string(), "video".to_string(), "audio".to_string()],
        output_types: vec!["text".to_string(), "audio".to_string()],
        billing_mode: None,
        input_price: None,
        cached_price: None,
        output_price: None,
        display_name: None,
        temperature: None,
        top_p: None,
        protocol: None,
        base_url: None,
        thinking_default: None,
        thinking_max: None,
        proxy: None,
    };

    cfg.upsert_model_config("ai-hub", custom_model.clone()).unwrap();

    let p = &cfg.providers["ai-hub"];
    let fetched = p.get_model_config("omni-v1");
    assert_eq!(fetched, custom_model);
    assert!(p.models.contains(&"omni-v1".to_string()));

    let all_models = p.list_all_models();
    assert_eq!(all_models.len(), 2);
    assert_eq!(all_models[0].name, "default-chat");
    assert_eq!(all_models[1].name, "omni-v1");

    // 3. Update provider
    cfg.update_provider("ai-hub", "https://new.aihub.com", "omni-v1", "round_robin").unwrap();
    assert_eq!(cfg.providers["ai-hub"].base_url, "https://new.aihub.com");
    assert_eq!(cfg.providers["ai-hub"].default_model, "omni-v1");
    assert_eq!(cfg.providers["ai-hub"].strategy, "round_robin");

    // 4. Test serialization and deserialization
    let serialized = toml::to_string_pretty(&cfg).unwrap();
    let deserialized: ConfigFile = toml::from_str(&serialized).unwrap();
    let des_p = &deserialized.providers["ai-hub"];
    assert_eq!(des_p.default_model, "omni-v1");
    let des_omni = des_p.get_model_config("omni-v1");
    assert_eq!(des_omni.context_window, "2M");
    assert_eq!(des_omni.max_output, "64K");
    assert_eq!(des_omni.input_types.len(), 4);
    assert_eq!(des_omni.output_types.len(), 2);

    // 5. Remove model (must switch default model before removing active default)
    let del_active_err = cfg.remove_model("ai-hub", "omni-v1");
    assert!(del_active_err.is_err());

    cfg.set_default_model("ai-hub", "default-chat").unwrap();
    let removed = cfg.remove_model("ai-hub", "omni-v1").unwrap();
    assert!(removed);
    assert!(!cfg.providers["ai-hub"].models.contains(&"omni-v1".to_string()));
}

#[test]
fn test_config_strict_load_and_atomic_save() {
    // 1. Specified non-existent config path must return error
    let res = ConfigFile::load_or_default(Some("non_existent_path_12345.toml"));
    assert!(res.is_err());

    // 2. Test atomic save and read back
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join(format!("test_atomic_config_{}.toml", std::process::id()));
    let temp_file_str = temp_file.to_str().unwrap();

    let mut cfg = ConfigFile::default();
    cfg.gateway.bind = "0.0.0.0:9999".to_string();
    cfg.add_provider("atomic-p", "https://api.atomic.com", "m-atomic", "priority");
    cfg.save_to_path(temp_file_str).unwrap();

    let loaded = ConfigFile::load_or_default(Some(temp_file_str)).unwrap();
    assert_eq!(loaded.gateway.bind, "0.0.0.0:9999");
    assert_eq!(loaded.providers["atomic-p"].default_model, "m-atomic");

    let _ = std::fs::remove_file(temp_file);
}

#[test]
fn test_cli_upgrade_command_parsing() {
    let u_check = Cli::try_parse_from(["ponyllm", "upgrade", "--check"]).unwrap();
    match u_check.command {
        Commands::Upgrade { check, force, dry_run, version } => {
            assert!(check);
            assert!(!force);
            assert!(!dry_run);
            assert_eq!(version, None);
        }
        _ => panic!("Expected Upgrade check"),
    }

    let u_force = Cli::try_parse_from([
        "ponyllm", "update", "--force", "--dry-run", "--version", "v0.3.0",
    ]).unwrap();
    match u_force.command {
        Commands::Upgrade { check, force, dry_run, version } => {
            assert!(!check);
            assert!(force);
            assert!(dry_run);
            assert_eq!(version, Some("v0.3.0".to_string()));
        }
        _ => panic!("Expected Upgrade force with version"),
    }
}

#[test]
fn test_upgrade_version_comparison_and_platform_detection() {
    use ponyllm_cli::upgrade::{detect_target_asset_name, is_newer_version, parse_version_triplet};

    // Platform detection
    let (asset_name, binary_name, _is_zip) = detect_target_asset_name().unwrap();
    assert!(!asset_name.is_empty());
    assert!(!binary_name.is_empty());

    // Version triplet parser
    assert_eq!(parse_version_triplet("0.2.1"), Some((0, 2, 1)));
    assert_eq!(parse_version_triplet("v1.5.10"), Some((1, 5, 10)));
    assert_eq!(parse_version_triplet("v2.0.0-rc1"), Some((2, 0, 0)));

    // Newer version checks
    assert!(is_newer_version("0.2.1", "0.2.2"));
    assert!(is_newer_version("0.2.1", "0.3.0"));
    assert!(is_newer_version("v0.2.1", "v1.0.0"));
    assert!(!is_newer_version("0.2.1", "0.2.1"));
    assert!(!is_newer_version("v0.3.0", "v0.2.1"));
    assert!(!is_newer_version("1.0.0", "0.9.9"));
}

#[test]
fn test_upgrade_zip_and_targz_extraction() {
    use ponyllm_cli::upgrade::{extract_targz, extract_zip};
    use std::io::Write;

    let temp_dir = tempfile::tempdir().unwrap();

    // 1. Create a mock zip archive in memory
    let mut zip_buf = Vec::new();
    {
        let mut zip_writer = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip_writer.start_file("mock_ponyllm.exe", options).unwrap();
        zip_writer.write_all(b"MOCK_BINARY_DATA").unwrap();
        zip_writer.finish().unwrap();
    }

    let extracted_zip_path = extract_zip(&zip_buf, "mock_ponyllm.exe", temp_dir.path()).unwrap();
    assert!(extracted_zip_path.exists());
    let content = std::fs::read(&extracted_zip_path).unwrap();
    assert_eq!(content, b"MOCK_BINARY_DATA");

    // 2. Create a mock tar.gz archive in memory
    let mut targz_buf = Vec::new();
    {
        let gz_encoder = flate2::write::GzEncoder::new(&mut targz_buf, flate2::Compression::default());
        let mut tar_builder = tar::Builder::new(gz_encoder);
        let mut header = tar::Header::new_gnu();
        header.set_path("mock_ponyllm").unwrap();
        header.set_size(16);
        header.set_mode(0o755);
        header.set_cksum();
        tar_builder.append(&header, &b"MOCK_TAR_GZ_DATA"[..]).unwrap();
        tar_builder.finish().unwrap();
    }

    let extracted_tar_path = extract_targz(&targz_buf, "mock_ponyllm", temp_dir.path()).unwrap();
    assert!(extracted_tar_path.exists());
    let tar_content = std::fs::read(&extracted_tar_path).unwrap();
    assert_eq!(tar_content, b"MOCK_TAR_GZ_DATA");
}

#[test]
fn test_upgrade_candidate_urls_primary_first_and_download_timeout() {
    use ponyllm_cli::upgrade::{
        build_candidate_urls, build_download_client, API_TIMEOUT_SECS, DOWNLOAD_TIMEOUT_SECS,
    };

    let primary = "https://github.com/lanhui100/ponyllm/releases/download/v0.2.40/ponyllm-linux-x86_64.tar.gz";
    let urls = build_candidate_urls(primary);
    assert_eq!(urls.len(), 3);
    assert_eq!(urls[0], primary);
    assert!(urls[1].ends_with(primary));
    assert!(urls[2].ends_with(primary));

    // 下载超时必须显著大于 API 超时，否则慢链路必失败（回归 2026-09-09 升级失败）
    assert!(DOWNLOAD_TIMEOUT_SECS >= 10 * API_TIMEOUT_SECS);
    // 下载 client 可正常构建
    let _client = build_download_client().unwrap();
}

#[test]
fn test_secure_api_key_generation_and_cli_auth_commands() {
    use ponyllm_cli::config::generate_secure_api_key;

    // 1. Verify high-entropy key algorithm
    let key1 = generate_secure_api_key();
    let key2 = generate_secure_api_key();
    assert!(key1.starts_with("sk-pony-"));
    assert!(key2.starts_with("sk-pony-"));
    assert_ne!(key1, key2);
    assert_eq!(key1.len(), 8 + 32); // "sk-pony-" (8) + 32 hex chars

    // 2. Verify CLI Commands::Auth parsing
    let cli_auth_view = Cli::try_parse_from(["ponyllm", "auth"]).unwrap();
    match cli_auth_view.command {
        Commands::Auth { config, key, rotate } => {
            assert_eq!(config, None);
            assert_eq!(key, None);
            assert!(!rotate);
        }
        _ => panic!("Expected Auth command"),
    }

    let cli_auth_rotate = Cli::try_parse_from(["ponyllm", "auth", "--rotate"]).unwrap();
    match cli_auth_rotate.command {
        Commands::Auth { config, key, rotate } => {
            assert_eq!(config, None);
            assert_eq!(key, None);
            assert!(rotate);
        }
        _ => panic!("Expected Auth command"),
    }

    let cli_auth_set = Cli::try_parse_from(["ponyllm", "auth", "-c", "custom.toml", "sk-custom-secret"]).unwrap();
    match cli_auth_set.command {
        Commands::Auth { config, key, rotate } => {
            assert_eq!(config, Some("custom.toml".to_string()));
            assert_eq!(key, Some("sk-custom-secret".to_string()));
            assert!(!rotate);
        }
        _ => panic!("Expected Auth command"),
    }

    // 3. Verify KeyCommands::Gateway parsing
    let cli_key_gw = Cli::try_parse_from(["ponyllm", "key", "gateway", "sk-another-token"]).unwrap();
    match cli_key_gw.command {
        Commands::Key(KeyCommands::Gateway { config, key, rotate }) => {
            assert_eq!(config, None);
            assert_eq!(key, Some("sk-another-token".to_string()));
            assert!(!rotate);
        }
        _ => panic!("Expected KeyCommands::Gateway"),
    }

    // 4. Verify Commands::Status parsing with config and api_key overrides
    let cli_status_default = Cli::try_parse_from(["ponyllm", "status"]).unwrap();
    match cli_status_default.command {
        Commands::Status { config, gateway_url, api_key } => {
            assert_eq!(config, None);
            assert_eq!(gateway_url, None);
            assert_eq!(api_key, None);
        }
        _ => panic!("Expected Status command"),
    }

    let cli_status_custom = Cli::try_parse_from(["ponyllm", "status", "-c", "my-pony.toml", "-g", "http://127.0.0.1:9090", "--api-key", "sk-test-key"]).unwrap();
    match cli_status_custom.command {
        Commands::Status { config, gateway_url, api_key } => {
            assert_eq!(config, Some("my-pony.toml".to_string()));
            assert_eq!(gateway_url, Some("http://127.0.0.1:9090".to_string()));
            assert_eq!(api_key, Some("sk-test-key".to_string()));
        }
        _ => panic!("Expected Status command"),
    }
}

#[test]
fn test_gateway_auth_action_safety_rules() {
    use ponyllm_cli::config::{parse_gateway_auth_action, GatewayAuthAction};

    // 1. Default (no args, no rotate) must be read-only Show
    assert_eq!(
        parse_gateway_auth_action(None, false),
        GatewayAuthAction::Show
    );

    // 2. Explicit show or get must be Show
    assert_eq!(
        parse_gateway_auth_action(Some("show"), false),
        GatewayAuthAction::Show
    );
    assert_eq!(
        parse_gateway_auth_action(Some("GET"), false),
        GatewayAuthAction::Show
    );

    // 3. User typing "list" must be intercepted as MisdirectedList, never as Set("list")
    assert_eq!(
        parse_gateway_auth_action(Some("list"), false),
        GatewayAuthAction::MisdirectedList
    );
    assert_eq!(
        parse_gateway_auth_action(Some("LIST"), true),
        GatewayAuthAction::MisdirectedList
    );

    // 4. Rotate flag or keywords must map to Rotate
    assert_eq!(
        parse_gateway_auth_action(None, true),
        GatewayAuthAction::Rotate
    );
    assert_eq!(
        parse_gateway_auth_action(Some("rotate"), false),
        GatewayAuthAction::Rotate
    );
    assert_eq!(
        parse_gateway_auth_action(Some("generate"), false),
        GatewayAuthAction::Rotate
    );

    // 5. Explicit custom secret key must map to Set
    assert_eq!(
        parse_gateway_auth_action(Some("sk-pony-my-custom-key"), false),
        GatewayAuthAction::Set("sk-pony-my-custom-key".to_string())
    );

    // 6. User typing "agy" or "antigravity" must be intercepted as MisdirectedAgy, never as Set("agy")
    assert_eq!(
        parse_gateway_auth_action(Some("agy"), false),
        GatewayAuthAction::MisdirectedAgy
    );
    assert_eq!(
        parse_gateway_auth_action(Some("AGY"), true),
        GatewayAuthAction::MisdirectedAgy
    );
    assert_eq!(
        parse_gateway_auth_action(Some("antigravity"), false),
        GatewayAuthAction::MisdirectedAgy
    );
}

#[test]
fn test_model_thinking_spec_toml_and_cli() {
    use ponyllm_cli::config::ModelConfig;
    use ponyllm_protocol::common::ReasoningEffort;


    // 1. Inferred from model name (o3-mini)
    let m1 = ModelConfig::new("o3-mini");
    let spec1 = m1.thinking_spec();
    assert_eq!(spec1.default_effort, ReasoningEffort::Low);
    assert_eq!(spec1.max_effort, ReasoningEffort::Medium);

    // 2. Custom override via config
    let mut m2 = ModelConfig::new("custom-reasoner");
    m2.thinking_default = Some(ReasoningEffort::Medium);
    m2.thinking_max = Some(ReasoningEffort::High);
    let spec2 = m2.thinking_spec();
    assert_eq!(spec2.default_effort, ReasoningEffort::Medium);
    assert_eq!(spec2.max_effort, ReasoningEffort::High);

    // 3. TOML serialization and deserialization
    let toml_str = r#"
name = "claude-opus-5"
thinking_default = "medium"
thinking_max = "high"
"#;
    let decoded: ModelConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(decoded.thinking_default, Some(ReasoningEffort::Medium));
    assert_eq!(decoded.thinking_max, Some(ReasoningEffort::High));
    let decoded_spec = decoded.thinking_spec();
    assert_eq!(decoded_spec.resolve(None), ReasoningEffort::Medium);
    assert_eq!(decoded_spec.resolve(Some(ReasoningEffort::High)), ReasoningEffort::High);
    assert_eq!(decoded_spec.resolve(Some(ReasoningEffort::Off)), ReasoningEffort::Off);
}

#[tokio::test]
async fn test_web_subcommand_full_launch_lifecycle() {
    let port = 18987;
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("ponyllm.toml");
    std::fs::write(&cfg_path, "[gateway]\napi_key = \"\"\n").unwrap();

    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_ponyllm"))
        .args([
            "web",
            "--config",
            cfg_path.to_str().unwrap(),
            "--port",
            &port.to_string(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to spawn ponyllm web");

    let client = reqwest::Client::new();
    let health_url = format!("http://127.0.0.1:{}/health", port);
    let app_url = format!("http://127.0.0.1:{}/app/", port);
    let mut ready = false;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Ok(resp) = client.get(&health_url).send().await {
            if resp.status().is_success() {
                ready = true;
                break;
            }
        }
    }
    assert!(ready, "Server failed to start on port {}", port);

    let app_resp = client.get(&app_url).send().await.expect("Failed to reach /app/");
    assert!(
        app_resp.status().is_success() || app_resp.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "unexpected status: {}",
        app_resp.status()
    );

    let _ = cmd.kill();
    let _ = cmd.wait();
}

#[test]
fn test_key_auth_agy_cli_parsing() {
    // 1. `ponyllm key auth agy`
    let cli1 = Cli::try_parse_from(["ponyllm", "key", "auth", "agy"]).unwrap();
    match cli1.command {
        Commands::Key(KeyCommands::Auth { provider, id, priority, weight, port, no_browser, .. }) => {
            assert_eq!(provider, "agy");
            assert_eq!(id, None);
            assert_eq!(priority, 1);
            assert_eq!(weight, 10);
            assert_eq!(port, 51121);
            assert!(!no_browser);
        }
        _ => panic!("Expected Key Auth command"),
    }

    // 2. `ponyllm key auth agy account-main`
    let cli2 = Cli::try_parse_from([
        "ponyllm", "key", "auth", "agy", "account-main", "-P", "2", "-W", "20", "--no-browser", "--port", "51130"
    ]).unwrap();
    match cli2.command {
        Commands::Key(KeyCommands::Auth { provider, id, priority, weight, port, no_browser, .. }) => {
            assert_eq!(provider, "agy");
            assert_eq!(id, Some("account-main".to_string()));
            assert_eq!(priority, 2);
            assert_eq!(weight, 20);
            assert_eq!(port, 51130);
            assert!(no_browser);
        }
        _ => panic!("Expected Key Auth command"),
    }

    // 3. Alias: `ponyllm key auth-agy my-account`
    let cli3 = Cli::try_parse_from(["ponyllm", "key", "auth-agy", "my-account"]).unwrap();
    match cli3.command {
        Commands::Key(KeyCommands::Auth { provider, id, .. }) => {
            // When using alias `auth-agy`, the first positional goes into `provider`
            // and `id` is None, which our handler normalizes.
            assert_eq!(provider, "my-account");
            assert_eq!(id, None);
        }
        _ => panic!("Expected Key Auth command via alias"),
    }
}

static OAUTH_ENV_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn test_handle_key_auth_agy_full_flow() {
    let _guard = OAUTH_ENV_MUTEX.lock().await;
    use axum::{routing::post, Json, Router};
    use serde_json::json;

    // 1. Start mock Google OAuth server
    let oauth_app = Router::new().route(
        "/token",
        post(|axum::Form(params): axum::Form<std::collections::HashMap<String, String>>| async move {
            assert_eq!(params.get("code").map(|s| s.as_str()), Some("mock-code-success"));
            Json(json!({
                "access_token": "ya29.mock-access",
                "refresh_token": "1//0mock-refresh-token-e2e",
                "expires_in": 3600,
                "id_token": "eyJhbGciOiJSUzI1NiJ9.eyJlbWFpbCI6ImFneS5kZXZlbG9wZXJAZ21haWwuY29tIiwic3ViIjoiOTk5In0.sig"
            }))
        }),
    );

    let oauth_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let oauth_port = oauth_listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(oauth_listener, oauth_app).await.unwrap();
    });

    std::env::set_var(
        "ANTIGRAVITY_OAUTH_TOKEN_URL_OVERRIDE",
        format!("http://127.0.0.1:{}/token", oauth_port),
    );
    std::env::set_var("PONYLLM_NON_INTERACTIVE", "1");

    // 2. Setup temp config file
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("ponyllm.toml");
    std::fs::write(&cfg_path, "[gateway]\nbind = \"127.0.0.1:8080\"\n").unwrap();

    let callback_port = 51188;
    let cfg_str = cfg_path.to_str().unwrap().to_string();

    // 3. Trigger handle_key_auth_agy in background
    let auth_task = tokio::spawn(async move {
        ponyllm_cli::oauth_agy::handle_key_auth_agy(
            "agy",
            Some("my-ag-custom-account"),
            1,
            15,
            callback_port,
            true, // no_browser
            Some(&cfg_str),
            None,
        )
        .await
    });

    // 4. Send callback request simulating Google OAuth redirect
    let client = reqwest::Client::new();
    let mut resp = None;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        for p in callback_port..callback_port + 10 {
            let callback_url = format!(
                "http://127.0.0.1:{}/oauth2callback?code=mock-code-success&state=123",
                p
            );
            if let Ok(r) = client.get(&callback_url).send().await {
                if r.status().is_success() {
                    resp = Some(r);
                    break;
                }
            }
        }
        if resp.is_some() {
            break;
        }
    }
    assert!(resp.is_some(), "Failed to send callback to bound callback port");

    // 5. Await auth completion
    let res = auth_task.await.expect("Task panicked");
    assert!(res.is_ok(), "handle_key_auth_agy failed: {:?}", res.err());

    // 6. Verify config file written
    let updated_cfg = ConfigFile::load_or_default(cfg_path.to_str()).expect("Failed to reload config");
    let agy_provider = updated_cfg.providers.get("antigravity").expect("antigravity provider missing");
    let key = agy_provider.keys.iter().find(|k| k.id == "my-ag-custom-account").expect("Key not found");
    assert_eq!(key.api_key, "1//0mock-refresh-token-e2e");
    assert_eq!(key.priority, 1);
    assert_eq!(key.weight, 15);

    std::env::remove_var("ANTIGRAVITY_OAUTH_TOKEN_URL_OVERRIDE");
    std::env::remove_var("PONYLLM_NON_INTERACTIVE");
}

#[test]
fn test_provider_add_agy_cli_parsing() {
    // 1. `ponyllm provider add agy`
    let cli1 = Cli::try_parse_from(["ponyllm", "provider", "add", "agy"]).unwrap();
    match cli1.command {
        Commands::Provider(ProviderCommands::Add { name, id, priority, weight, port, no_browser, .. }) => {
            assert_eq!(name, "agy");
            assert_eq!(id, None);
            assert_eq!(priority, 1);
            assert_eq!(weight, 10);
            assert_eq!(port, 51121);
            assert!(!no_browser);
        }
        _ => panic!("Expected Provider Add command"),
    }

    // 2. `ponyllm provider add agy --id agy-label -P 2 -W 25 --port 51180 --no-browser`
    let cli2 = Cli::try_parse_from([
        "ponyllm", "provider", "add", "agy", "--id", "agy-label", "-P", "2", "-W", "25", "--port", "51180", "--no-browser"
    ]).unwrap();
    match cli2.command {
        Commands::Provider(ProviderCommands::Add { name, id, priority, weight, port, no_browser, .. }) => {
            assert_eq!(name, "agy");
            assert_eq!(id, Some("agy-label".to_string()));
            assert_eq!(priority, 2);
            assert_eq!(weight, 25);
            assert_eq!(port, 51180);
            assert!(no_browser);
        }
        _ => panic!("Expected Provider Add command with flags"),
    }

    // 3. `ponyllm provider add antigravity`
    let cli3 = Cli::try_parse_from(["ponyllm", "provider", "add", "antigravity"]).unwrap();
    match cli3.command {
        Commands::Provider(ProviderCommands::Add { name, .. }) => {
            assert_eq!(name, "antigravity");
        }
        _ => panic!("Expected Provider Add command"),
    }
}

#[tokio::test]
async fn test_provider_add_agy_multi_account_append() {
    let _guard = OAUTH_ENV_MUTEX.lock().await;
    use axum::{routing::post, Json, Router};
    use serde_json::json;

    let oauth_app = Router::new().route(
        "/token",
        post(|axum::Form(params): axum::Form<std::collections::HashMap<String, String>>| async move {
            let code = params.get("code").map(|s| s.as_str()).unwrap_or("");
            Json(json!({
                "access_token": "ya29.mock-access",
                "refresh_token": format!("1//0mock-refresh-{}", code),
                "expires_in": 3600,
                "id_token": "eyJhbGciOiJSUzI1NiJ9.eyJlbWFpbCI6ImFneS5kZXZlbG9wZXJAZ21haWwuY29tIiwic3ViIjoiOTk5In0.sig"
            }))
        }),
    );

    let oauth_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let oauth_port = oauth_listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(oauth_listener, oauth_app).await.unwrap();
    });

    std::env::set_var(
        "ANTIGRAVITY_OAUTH_TOKEN_URL_OVERRIDE",
        format!("http://127.0.0.1:{}/token", oauth_port),
    );
    std::env::set_var("PONYLLM_NON_INTERACTIVE", "1");

    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("ponyllm.toml");
    std::fs::write(&cfg_path, "[gateway]\nbind = \"127.0.0.1:8080\"\n").unwrap();
    let cfg_str = cfg_path.to_str().unwrap().to_string();

    // 1. First account: ponyllm provider add agy --id agy-acc-1 -P 1 -W 10
    let p1 = 51210;
    let cfg_str_clone = cfg_str.clone();
    let task1 = tokio::spawn(async move {
        ponyllm_cli::oauth_agy::handle_key_auth_agy(
            "agy",
            Some("agy-acc-1"),
            1,
            10,
            p1,
            true,
            Some(&cfg_str_clone),
            None,
        )
        .await
    });

    let client = reqwest::Client::new();
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(r) = client.get(format!("http://127.0.0.1:{}/oauth2callback?code=acc1", p1)).send().await {
            if r.status().is_success() { break; }
        }
    }
    task1.await.unwrap().unwrap();

    // 2. Second account: ponyllm provider add antigravity --id agy-acc-2 -P 2 -W 20
    let p2 = 51230;
    let cfg_str_clone2 = cfg_str.clone();
    let task2 = tokio::spawn(async move {
        ponyllm_cli::oauth_agy::handle_key_auth_agy(
            "antigravity",
            Some("agy-acc-2"),
            2,
            20,
            p2,
            true,
            Some(&cfg_str_clone2),
            None,
        )
        .await
    });

    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(r) = client.get(format!("http://127.0.0.1:{}/oauth2callback?code=acc2", p2)).send().await {
            if r.status().is_success() { break; }
        }
    }
    task2.await.unwrap().unwrap();

    // 3. Verify multi-account pool in config
    let updated = ConfigFile::load_or_default(cfg_path.to_str()).expect("Failed to load config");
    let provider = updated.providers.get("antigravity").expect("antigravity provider missing");
    assert_eq!(provider.keys.len(), 2, "Expected 2 keys in antigravity provider pool");

    let k1 = provider.keys.iter().find(|k| k.id == "agy-acc-1").expect("k1 not found");
    assert_eq!(k1.api_key, "1//0mock-refresh-acc1");
    assert_eq!(k1.priority, 1);
    assert_eq!(k1.weight, 10);

    let k2 = provider.keys.iter().find(|k| k.id == "agy-acc-2").expect("k2 not found");
    assert_eq!(k2.api_key, "1//0mock-refresh-acc2");
    assert_eq!(k2.priority, 2);
    assert_eq!(k2.weight, 20);

    std::env::remove_var("ANTIGRAVITY_OAUTH_TOKEN_URL_OVERRIDE");
    std::env::remove_var("PONYLLM_NON_INTERACTIVE");
}

#[tokio::test]
async fn test_provider_add_agy_proxy_auto_detection_and_persistence() {
    let _guard = OAUTH_ENV_MUTEX.lock().await;
    use axum::{routing::post, Json, Router};
    use serde_json::json;

    let oauth_app = Router::new().route(
        "/token",
        post(|axum::Form(params): axum::Form<std::collections::HashMap<String, String>>| async move {
            let code = params.get("code").map(|s| s.as_str()).unwrap_or("");
            Json(json!({
                "access_token": "ya29.mock-access",
                "refresh_token": format!("1//0mock-refresh-{}", code),
                "expires_in": 3600,
                "id_token": "eyJhbGciOiJSUzI1NiJ9.eyJlbWFpbCI6InByb3h5LnRlc3RAZ21haWwuY29tIiwic3ViIjoiOTk5In0.sig"
            }))
        }),
    );

    let oauth_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let oauth_port = oauth_listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(oauth_listener, oauth_app).await.unwrap();
    });

    std::env::set_var(
        "ANTIGRAVITY_OAUTH_TOKEN_URL_OVERRIDE",
        format!("http://127.0.0.1:{}/token", oauth_port),
    );
    std::env::set_var("PONYLLM_NON_INTERACTIVE", "1");
    std::env::set_var("https_proxy", "http://127.0.0.1:8899");

    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("ponyllm.toml");
    std::fs::write(&cfg_path, "[gateway]\nbind = \"127.0.0.1:8080\"\n").unwrap();
    let cfg_str = cfg_path.to_str().unwrap().to_string();

    let p = 51250;
    let cfg_str_clone = cfg_str.clone();
    let task = tokio::spawn(async move {
        ponyllm_cli::oauth_agy::handle_key_auth_agy(
            "agy",
            Some("agy-proxy-acc"),
            1,
            10,
            p,
            true,
            Some(&cfg_str_clone),
            None,
        )
        .await
    });

    let client = reqwest::Client::new();
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(r) = client.get(format!("http://127.0.0.1:{}/oauth2callback?code=pxy1", p)).send().await {
            if r.status().is_success() { break; }
        }
    }
    task.await.unwrap().unwrap();

    let updated = ConfigFile::load_or_default(cfg_path.to_str()).expect("Failed to load config");
    let provider = updated.providers.get("antigravity").expect("antigravity provider missing");
    assert_eq!(provider.proxy.as_deref(), Some("http://127.0.0.1:8899"));

    std::env::remove_var("ANTIGRAVITY_OAUTH_TOKEN_URL_OVERRIDE");
    std::env::remove_var("PONYLLM_NON_INTERACTIVE");
}
