//! HS256 JWT (B001): self-hosted JWT built on `ring::hmac` + `base64` +
//! `serde_json`, with a fixed algorithm whitelist (`HS256` only — no `alg`
//! negotiation).
//!
//! Verification order (contract `red_jwt_tests.rs`): structural/decoding
//! faults → `exp` (→ `Expired`) → `iss`/`iat`/`role` (→ `InvalidToken`) →
//! HMAC mismatch (→ `InvalidSignature`). HMAC comparison is constant-time via
//! `ring::hmac::verify`; we never fall back to a variable-time compare.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// JWT claims payload (B001 contract).
///
/// `role` is the lowercase wire form (`"admin"` | `"user"`), matching
/// [`crate::user::UserRole`] serde names; `tv` is the user's `token_version`
/// for stateless revocation (short TTL + `tv` compare + `enabled` live check).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// Subject: user id (`UserEntry.id`).
    pub sub: String,
    /// Login username (`UserEntry.username`).
    pub username: String,
    /// Access role, lowercase wire form (`"admin"` | `"user"`).
    pub role: String,
    /// Token version at issuance; changed on password rotation/revocation.
    pub tv: u64,
    /// Issued-at (UNIX seconds).
    pub iat: i64,
    /// Expiry (UNIX seconds).
    pub exp: i64,
    /// Issuer; `verify` rejects tokens whose `iss` differs.
    pub iss: String,
}

/// Fixed JOSE header for every token this module signs (`alg=HS256`,
/// `typ=JWT`). `verify` rejects any other `alg` (algorithm-confusion guard).
const FIXED_HEADER: &[u8] = br#"{"alg":"HS256","typ":"JWT"}"#;
/// Accepted `alg` values (whitelist — only HS256 may ever pass).
const ACCEPTED_ALG: &str = "HS256";
/// Accepted `role` wire values (claims validation; anything else is invalid).
const ACCEPTED_ROLES: [&str; 2] = ["admin", "user"];
/// Clock-skew allowance for the `iat` future check (seconds). Kept small so a
/// tampered `iat` is rejected while a benign clock skew stays tolerable.
const IAT_SKEW_SECS: i64 = 300;

/// Sign claims into a compact HS256 JWT (`header.payload.signature`).
///
/// Builds `{"alg":"HS256","typ":"JWT"}`, MACs `base64url(header).base64url(payload)`
/// with `ring::hmac::sign` under `HMAC_SHA256`, and appends the base64url MAC.
pub fn sign(claims: &Claims, secret: &[u8]) -> Result<String, JwtError> {
    let payload = serde_json::to_vec(claims)
        .map_err(|e| JwtError::Other(format!("claims serialization: {e}")))?;
    let header_b64 = URL_SAFE_NO_PAD.encode(FIXED_HEADER);
    let payload_b64 = URL_SAFE_NO_PAD.encode(&payload);
    let signing_input = format!("{header_b64}.{payload_b64}");

    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret);
    let mac = ring::hmac::sign(&key, signing_input.as_bytes());
    let sig_b64 = URL_SAFE_NO_PAD.encode(mac.as_ref());

    Ok(format!("{signing_input}.{sig_b64}"))
}

/// Verify a compact HS256 JWT and recover its claims.
///
/// Fail-closed ordering (contract): structural/decoding faults → `InvalidToken`
/// or `Other`; `exp` in the past → `Expired`; `iss` mismatch / future `iat` /
/// non-whitelisted `role` → `InvalidToken`; then constant-time HMAC verification
/// via `ring::hmac::verify` → `InvalidSignature` on mismatch.
pub fn verify(token: &str, secret: &[u8], issuer: &str) -> Result<Claims, JwtError> {
    // 1. Structural: exactly three segments, header + payload decodable, header
    //    `alg` on the fixed whitelist.
    let segments: Vec<&str> = token.split('.').collect();
    if segments.len() != 3 {
        return Err(JwtError::InvalidToken);
    }
    let header = URL_SAFE_NO_PAD
        .decode(segments[0])
        .map_err(|_| JwtError::InvalidToken)?;
    let payload = URL_SAFE_NO_PAD
        .decode(segments[1])
        .map_err(|_| JwtError::InvalidToken)?;

    let header_json: serde_json::Value =
        serde_json::from_slice(&header).map_err(|_| JwtError::InvalidToken)?;
    let alg = header_json
        .get("alg")
        .and_then(serde_json::Value::as_str)
        .ok_or(JwtError::InvalidToken)?;
    if alg != ACCEPTED_ALG {
        return Err(JwtError::InvalidToken);
    }

    let claims: Claims = serde_json::from_slice(&payload).map_err(|_| JwtError::InvalidToken)?;

    // 2. Expiry: `exp` in the past (or equal to now) is rejected.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if claims.exp <= now {
        return Err(JwtError::Expired);
    }

    // 3. Issuer / issued-at / role sanity (fail-closed, before trusting HMAC).
    if claims.iss != issuer {
        return Err(JwtError::InvalidToken);
    }
    if claims.iat > now + IAT_SKEW_SECS {
        return Err(JwtError::InvalidToken);
    }
    if !ACCEPTED_ROLES.contains(&claims.role.as_str()) {
        return Err(JwtError::InvalidToken);
    }

    // 4. Constant-time HMAC verification over the exact signing input — the
    //    signature must match `header.payload` under `secret`.
    let signature = URL_SAFE_NO_PAD
        .decode(segments[2])
        .map_err(|_| JwtError::InvalidToken)?;
    let signing_input = format!("{}.{}", segments[0], segments[1]);
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret);
    ring::hmac::verify(&key, signing_input.as_bytes(), &signature)
        .map_err(|_| JwtError::InvalidSignature)?;

    Ok(claims)
}

/// JWT verification errors (B001 contract, frozen).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum JwtError {
    /// `exp` is in the past.
    #[error("token expired")]
    Expired,
    /// HMAC signature does not match `secret`.
    #[error("invalid signature")]
    InvalidSignature,
    /// Structural fault: malformed encoding, wrong segment count, unexpected
    /// alg, or `iss` mismatch.
    #[error("invalid token")]
    InvalidToken,
    /// Unclassifiable internal error (e.g. serialization/decoding backend).
    #[error("jwt error: {0}")]
    Other(String),
}
