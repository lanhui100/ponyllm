//! 红相契约测试（B001）：PBKDF2-HMAC-SHA256 口令哈希模块（`ponyllm_core::password`）。
//!
//! 冻结契约（protocol-agent 锁定，Lead 契约为准）：
//! ```text
//! pub const PBKDF2_ITERATIONS: u32 = 210_000;
//! pub fn generate_salt() -> [u8; 16];
//! pub fn hash_password(password: &str, salt: &[u8], iterations: u32) -> String;
//! pub fn verify_password(password: &str, encoded: &str) -> bool;
//! ```
//! PHC 输出格式 `$pbkdf2-sha256$i=<iters>$<salt_b64>$<hash_b64>`；接口不可失败（无 Result）。
//!
//! 红相状态：契约尚未实现，本文件断言预期 FAIL（空桩即 panic / 断言失败）。
//! 零假测试律：每条用例都是真实断言。

use ponyllm_core::password::{generate_salt, hash_password, verify_password, PBKDF2_ITERATIONS};

/// 契约：hash 后 verify 对正确口令返回 true。
#[test]
fn hash_then_verify_correct_password_returns_true() {
    // Arrange
    let password = "correct horse battery staple";
    let salt = generate_salt();

    // Act
    let phc = hash_password(password, &salt, PBKDF2_ITERATIONS);

    // Assert
    assert!(
        verify_password(password, &phc),
        "verify must accept the exact password that was hashed"
    );
}

/// 契约：错误口令 verify 必须返回 false（绝不误放行）。
#[test]
fn wrong_password_is_rejected() {
    // Arrange
    let salt = generate_salt();
    let phc = hash_password("s3cret-pass", &salt, PBKDF2_ITERATIONS);

    // Act & Assert
    assert!(
        !verify_password("wrong-pass", &phc),
        "verify must reject a wrong password"
    );
}

/// 契约：盐唯一 —— 同一口令、不同盐必须产出不同哈希（盐参与派生，防彩虹表/同级重放）。
#[test]
fn different_salt_yields_different_hash() {
    // Arrange —— 用两个显式不同的盐保证确定性（非随机、零 flaky）
    let salt_a = [0x11u8; 16];
    let salt_b = [0x22u8; 16];
    let password = "same-password";

    // Act
    let a = hash_password(password, &salt_a, PBKDF2_ITERATIONS);
    let b = hash_password(password, &salt_b, PBKDF2_ITERATIONS);

    // Assert
    assert_ne!(a, b, "two different salts must yield different PHC outputs");
    assert!(verify_password(password, &a), "hash a must verify");
    assert!(verify_password(password, &b), "hash b must verify");
}

/// 契约：同一盐 + 同一口令必须可复现（确定性哈希，便于运维诊断）。
#[test]
fn same_salt_and_password_is_deterministic() {
    // Arrange / Act
    let salt = [0x42u8; 16];
    let a = hash_password("deterministic", &salt, PBKDF2_ITERATIONS);
    let b = hash_password("deterministic", &salt, PBKDF2_ITERATIONS);

    // Assert
    assert_eq!(a, b, "identical salt+password must produce identical PHC");
}

/// 契约：generate_salt 返回 16 字节随机源（元数据为 16 字节，防弱盐）。
#[test]
fn generate_salt_returns_16_bytes() {
    // Act
    let salt = generate_salt();

    // Assert
    assert_eq!(salt.len(), 16, "PBKDF2 salt must be 16 bytes");
}

/// 契约：PHC 格式必须含 `$pbkdf2-sha256$` 前缀及 i/盐/哈希三段。
#[test]
fn phc_format_has_pbkdf2_sha256_prefix() {
    // Arrange / Act
    let phc = hash_password("format-check", &[0u8; 16], PBKDF2_ITERATIONS);

    // Assert
    assert!(
        phc.starts_with("$pbkdf2-sha256$"),
        "PHC prefix missing: got `{phc}`"
    );

    let remainder = phc.strip_prefix("$pbkdf2-sha256$").unwrap();
    let segments: Vec<&str> = remainder.split('$').collect();
    assert_eq!(
        segments.len(),
        3,
        "PHC must be `...$i=<iters>$<salt_b64>$<hash_b64>`, got {:?}",
        segments
    );
    assert!(
        segments[0].starts_with("i="),
        "iter segment must be `i=<iters>`: {phc}"
    );
    let iters: u64 = segments[0]
        .trim_start_matches("i=")
        .parse()
        .expect("iterations must be a u64");
    assert_eq!(
        iters,
        u64::from(PBKDF2_ITERATIONS),
        "iterations must be recorded verbatim"
    );
    assert!(
        !segments[1].is_empty(),
        "salt segment must not be empty: {phc}"
    );
    assert!(
        !segments[2].is_empty(),
        "hash segment must not be empty: {phc}"
    );
}

/// 契约：verify 对畸形/垃圾 PHC 字符串必须返回 false 而非 panic（fail-closed）。
#[test]
fn verify_rejects_garbage_phc_without_panicking() {
    // Act & Assert
    assert!(!verify_password("anything", "not-a-valid-phc-format"));
    assert!(!verify_password("anything", ""));
    assert!(!verify_password("anything", "$pbkdf2-sha256$"));
}

/// 契约：接口不可变 —— 空口令也必须被可编程处理（hash 不 panic 且成对验证）。
#[test]
fn empty_password_hashes_and_verifies() {
    // Arrange / Act
    let salt = generate_salt();
    let phc = hash_password("", &salt, PBKDF2_ITERATIONS);

    // Assert
    assert!(
        verify_password("", &phc),
        "empty password must verify against its hash"
    );
}
