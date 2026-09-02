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
    let cli = Cli::try_parse_from(["ponyllm", "serve", "--config", "test.toml", "-a", "0.0.0.0", "-p", "9000", "--api-key", "sk-test-tok"]).unwrap();
    match cli.command {
        Commands::Serve { config, address, port, api_key, .. } => {
            assert_eq!(config, Some("test.toml".to_string()));
            assert_eq!(address, Some("0.0.0.0".to_string()));
            assert_eq!(port, Some(9000));
            assert_eq!(api_key, Some("sk-test-tok".to_string()));
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
        context_window: "2M".to_string(),
        max_output: "64K".to_string(),
        input_types: vec!["text".to_string(), "image".to_string(), "video".to_string(), "audio".to_string()],
        output_types: vec!["text".to_string(), "audio".to_string()],
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
