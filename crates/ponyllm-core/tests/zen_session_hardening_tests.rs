//! OpenCode Zen session hardening acceptance tests (Red phase).
//!
//! Validates:
//! 1. Downstream non-standard session IDs (e.g. "session-dsh-1234", "ses_" + 64 hex,
//!    special characters, too short / too long) are sanitized and discarded by
//!    `resolve_upstream_session`, automatically falling back to a compliant
//!    `ses_<26 alphanumeric>` format (total length 30).
//! 2. Downstream compliant `ses_<26 alphanumeric>` session IDs are preserved as-is.

use axum::http::{HeaderMap, HeaderValue};
use ponyllm_core::executor::resolve_upstream_session;

/// Helper to construct HeaderMap from key-value pairs
fn headers_from(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (k, v) in pairs {
        map.insert(
            axum::http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            HeaderValue::from_str(v).unwrap(),
        );
    }
    map
}

/// Helper to verify whether a session id conforms to `ses_<26 alphanumeric>` (total 30 chars)
fn is_valid_zen_session(s: &str) -> bool {
    if !s.starts_with("ses_") || s.len() != 30 {
        return false;
    }
    s[4..].chars().all(|c| c.is_ascii_alphanumeric())
}

#[test]
fn test_resolve_upstream_session_preserves_valid_zen_session() {
    // Exact 30 chars: "ses_" (4) + 26 base62 chars
    let valid_ses = "ses_0123456789abcdefABCDEF0123";
    assert_eq!(valid_ses.len(), 30);
    assert!(is_valid_zen_session(valid_ses));

    let headers = headers_from(&[("x-opencode-session", valid_ses)]);
    let resolved = resolve_upstream_session(&headers);
    assert_eq!(
        resolved, valid_ses,
        "Compliant ses_<26> must be preserved as-is"
    );

    // Also verify when passed via alias priority (e.g., x-session-affinity)
    let headers_alias = headers_from(&[("x-session-affinity", valid_ses)]);
    let resolved_alias = resolve_upstream_session(&headers_alias);
    assert_eq!(
        resolved_alias, valid_ses,
        "Compliant ses_<26> passed via alias header must also be preserved"
    );
}

#[test]
fn test_resolve_upstream_session_rejects_and_falls_back_on_malformed_sessions() {
    // 1. DSH legacy session format: "session-dsh-1234"
    let dsh_session = "session-dsh-1234";
    let headers = headers_from(&[("x-opencode-session", dsh_session)]);
    let resolved = resolve_upstream_session(&headers);
    assert_ne!(
        resolved, dsh_session,
        "DSH session format must not be passed upstream"
    );
    assert!(
        is_valid_zen_session(&resolved),
        "Must fall back to generated ses_<26>: got '{}'",
        resolved
    );

    // 2. ses_ + 64 hex characters (too long, not 26 chars)
    let hex_64 = "ses_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let headers = headers_from(&[("x-opencode-session", hex_64)]);
    let resolved = resolve_upstream_session(&headers);
    assert_ne!(
        resolved, hex_64,
        "Oversized 64-hex session must be rejected"
    );
    assert!(
        is_valid_zen_session(&resolved),
        "Must fall back to generated ses_<26>: got '{}'",
        resolved
    );

    // 3. ses_ + too short (e.g., 10 chars instead of 26)
    let too_short = "ses_short123";
    let headers = headers_from(&[("x-opencode-session", too_short)]);
    let resolved = resolve_upstream_session(&headers);
    assert_ne!(
        resolved, too_short,
        "Short session must be rejected"
    );
    assert!(
        is_valid_zen_session(&resolved),
        "Must fall back to generated ses_<26>: got '{}'",
        resolved
    );

    // 4. ses_ + 26 chars containing non-alphanumeric special characters
    let with_special_chars = "ses_0123456789!@#$%^&*()-+=~";
    let headers = headers_from(&[("x-opencode-session", with_special_chars)]);
    let resolved = resolve_upstream_session(&headers);
    assert_ne!(
        resolved, with_special_chars,
        "Session with special characters must be rejected"
    );
    assert!(
        is_valid_zen_session(&resolved),
        "Must fall back to generated ses_<26>: got '{}'",
        resolved
    );

    // 5. ses_ + 26 chars with whitespace/newlines
    let with_whitespace = "ses_0123456789 abcdefABCDEF01";
    let headers = headers_from(&[("x-opencode-session", with_whitespace)]);
    let resolved = resolve_upstream_session(&headers);
    assert_ne!(
        resolved, with_whitespace,
        "Session with whitespace must be rejected"
    );
    assert!(
        is_valid_zen_session(&resolved),
        "Must fall back to generated ses_<26>: got '{}'",
        resolved
    );
}
