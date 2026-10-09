//! Password hashing (B001): PBKDF2-HMAC-SHA256 via `ring::pbkdf2`, with a
//! PHC-style encoded output (`$pbkdf2-sha256$i=<iters>$<salt_b64>$<hash_b64>`).
//!
//! Design constraints (ADR `2026-10-09-web-user-jwt-and-token-system.md`):
//! - High iteration count + per-user random salt. Never reuse the fast
//!   `hash_gateway_key` path (SHA-256 single pass) for human passwords.
//! - `verify_password` is fail-closed: malformed/unsupported encodings return
//!   `false` and never panic; the derived-key comparison is constant-time via
//!   `ring::pbkdf2::verify`.

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use ring::pbkdf2::{self, PBKDF2_HMAC_SHA256};
use ring::rand::SecureRandom as _;
use std::num::NonZeroU32;

/// Default PBKDF2 iteration count for human passwords (OWASP 2023 guidance
/// tier). Callers may pass a different count to `hash_password`; this constant
/// is the canonical default for the login/registration paths.
pub const PBKDF2_ITERATIONS: u32 = 210_000;

/// PBKDF2-HMAC-SHA256 derived-key length in bytes (SHA-256 digest size).
const DERIVED_KEY_LEN: usize = 32;

/// Generate a fresh 16-byte random salt for one password hash.
///
/// Backed by the OS CSPRNG (`ring::rand::SystemRandom`); a `fill` failure is
/// effectively unreachable on supported platforms and treated as a hard error
/// rather than degrading to a weak salt.
pub fn generate_salt() -> [u8; 16] {
    let mut salt = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut salt)
        .expect("secure RNG failure while generating password salt");
    salt
}

/// Hash a password with PBKDF2-HMAC-SHA256 and return a PHC-format string:
/// `$pbkdf2-sha256$i=<iterations>$<salt_b64>$<hash_b64>`.
///
/// `iterations` is clamped to at least 1 (`ring` requires a non-zero count);
/// salt and derived key are base64-encoded (standard alphabet, unpadded).
pub fn hash_password(password: &str, salt: &[u8], iterations: u32) -> String {
    let iterations = NonZeroU32::new(iterations).unwrap_or(NonZeroU32::MIN);
    let mut derived = [0u8; DERIVED_KEY_LEN];
    pbkdf2::derive(
        PBKDF2_HMAC_SHA256,
        iterations,
        salt,
        password.as_bytes(),
        &mut derived,
    );
    format!(
        "$pbkdf2-sha256$i={iterations}${}${}",
        STANDARD_NO_PAD.encode(salt),
        STANDARD_NO_PAD.encode(derived),
    )
}

/// Verify a plaintext password against a stored PHC-encoded hash.
///
/// Parses `$pbkdf2-sha256$i=<iters>$<salt_b64>$<hash_b64>`, re-derives the key
/// with `ring::pbkdf2::verify` (constant-time comparison), and returns `false`
/// for any malformed/unsupported encoding (fail-closed, never panics).
pub fn verify_password(password: &str, encoded: &str) -> bool {
    let Some(rest) = encoded.strip_prefix("$pbkdf2-sha256$") else {
        return false;
    };
    let segments: Vec<&str> = rest.split('$').collect();
    if segments.len() != 3 {
        return false;
    }
    let Some(iters_str) = segments[0].strip_prefix("i=") else {
        return false;
    };
    let Ok(iterations) = iters_str.parse::<u32>() else {
        return false;
    };
    let Some(iterations) = NonZeroU32::new(iterations) else {
        return false;
    };

    // Accept both unpadded (what `hash_password` emits) and padded base64.
    let salt = decode_b64(segments[1]).unwrap_or_default();
    let expected = decode_b64(segments[2]).unwrap_or_default();
    if salt.is_empty() || expected.len() != DERIVED_KEY_LEN {
        return false;
    }

    pbkdf2::verify(
        PBKDF2_HMAC_SHA256,
        iterations,
        &salt,
        password.as_bytes(),
        &expected,
    )
    .is_ok()
}

/// Decode standard-alphabet base64, tolerating optional padding.
fn decode_b64(input: &str) -> Option<Vec<u8>> {
    STANDARD_NO_PAD
        .decode(input)
        .or_else(|_| STANDARD.decode(input))
        .ok()
}
