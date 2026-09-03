use std::fs;
use std::path::Path;
use ponyllm_core::discovery::resolve_config_path_from;

#[test]
fn test_explicit_path_takes_precedence() {
    let explicit = Path::new("custom/path/ponyllm.toml");
    let resolved = resolve_config_path_from(Some(explicit), None);
    assert_eq!(resolved, explicit);
}

#[test]
fn test_env_var_override() {
    std::env::set_var("PONYLLM_CONFIG", "/env/override/ponyllm.toml");
    let resolved = resolve_config_path_from(None, None);
    assert_eq!(resolved.to_str().unwrap(), "/env/override/ponyllm.toml");
    std::env::remove_var("PONYLLM_CONFIG");
}

#[test]
fn test_walk_upwards_from_child_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let root_config = root.join("ponyllm.toml");
    fs::write(&root_config, "test = true").unwrap();

    let child = root.join("a").join("b").join("c");
    fs::create_dir_all(&child).unwrap();

    // From child directory "a/b/c", it should detect the parent's ponyllm.toml!
    let resolved = resolve_config_path_from(None, Some(&child));
    assert_eq!(resolved, root_config);
}

#[test]
fn test_fallback_when_none_exists() {
    let temp = tempfile::tempdir().unwrap();
    let empty_dir = temp.path().join("empty");
    fs::create_dir_all(&empty_dir).unwrap();

    let resolved = resolve_config_path_from(None, Some(&empty_dir));
    // If not found in upwards walk or global, defaults to empty_dir/ponyllm.toml
    assert!(resolved.ends_with("ponyllm.toml"));
}
