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
