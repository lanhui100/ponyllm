use std::process::Command;
use tempfile::NamedTempFile;

#[test]
fn test_cli_user_subcommands_crud() {
    let temp_file = NamedTempFile::new().unwrap();
    let config_path = temp_file.path().to_str().unwrap();

    let exe = env!("CARGO_BIN_EXE_ponyllm");

    // 1. List users initially empty
    let output = Command::new(exe)
        .args(&["user", "list", "--config", config_path])
        .output()
        .expect("failed to execute ponyllm user list");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("暂无配置用户"));

    // 2. Add user
    let output = Command::new(exe)
        .args(&[
            "user",
            "add",
            "test_alice",
            "--name",
            "Alice",
            "--models",
            "gpt-4o-mini,deepseek/*",
            "--max-tokens",
            "50000",
            "--config",
            config_path,
        ])
        .output()
        .expect("failed to execute ponyllm user add");
    if !output.status.success() {
        eprintln!("STDERR: {}", String::from_utf8_lossy(&output.stderr));
        eprintln!("STDOUT: {}", String::from_utf8_lossy(&output.stdout));
    }
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("已创建"));

    // 3. List users contains alice
    let output = Command::new(exe)
        .args(&["user", "list", "--config", config_path])
        .output()
        .expect("failed to execute ponyllm user list");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("test_alice"));
    assert!(stdout.contains("Alice"));
    assert!(stdout.contains("50000"));

    // 4. Remove user
    let output = Command::new(exe)
        .args(&["user", "remove", "test_alice", "--config", config_path])
        .output()
        .expect("failed to execute ponyllm user remove");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("已移除"));

    // 5. List users empty again
    let output = Command::new(exe)
        .args(&["user", "list", "--config", config_path])
        .output()
        .expect("failed to execute ponyllm user list");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("暂无配置用户"));
}
