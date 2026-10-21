//! OpenCode Zen session hardening acceptance tests (CONTRACT-v1 C1, hex12 form).
//!
//! Frozen contract: `/home/dm/ponyllm/.dev-team/zen-rc/CONTRACT-v1.md` (C1 +
//! its evolution note). The rule is now (empirically verified 2026-10-21:
//! `ses_5e67a59ef81ehxVk8zG0o0Nd9h` → HTTP 200,
//! `ses_GQs9Ss1p7NsR7NoY1p7hRVA5sV` → HTTP 403 FreeTierError):
//!
//! * A **legal** upstream session is `^ses_[0-9a-f]{12}[0-9A-Za-z]{14}$` — `ses_`
//!   plus 12 lowercase-hex chars plus 14 alphanumeric (base62) chars, 30
//!   characters in total. The hex prefix is load-bearing: an all-alphanumeric
//!   26-char suffix whose first 12 chars are not lowercase hex is rejected
//!   upstream with HTTP 403.
//! * The **64-lowercase-hex shape** (`ses_` + 64 hex, the v1 misreading) is
//!   NON-STANDARD: upstream returns HTTP 403 for it, so it must be
//!   cleaned/discarded and replaced by a freshly generated compliant id.
//!
//! `resolve_upstream_session` therefore:
//! 1. preserves a downstream contract-valid
//!    `ses_<12 hex><14 alphanumeric>` session (including via the alias headers
//!    `x-session-affinity` / `x-session-id`), and
//! 2. discards every other shape (64-hex suffix, non-hex 12-char prefix,
//!    `ses-...`, `session-...`, special characters, whitespace, UUID, wrong
//!    length) and falls back to a generated id that satisfies the same contract
//!    shape.

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

/// CONTRACT-v1 C1 (hex12 form): legal session shape is
/// `^ses_[0-9a-f]{12}[0-9A-Za-z]{14}$` (30 chars) — first 12 payload chars are
/// lowercase hex, the trailing 14 are base62. Mirrors the upstream-side check in
/// `ponyllm_core::executor::is_valid_zen_session_format` so this file can never
/// drift looser than the implementation again.
fn is_valid_zen_session(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("ses_") else {
        return false;
    };
    let bytes = rest.as_bytes();
    bytes.len() == 26
        && bytes[..12]
            .iter()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        && bytes[12..].iter().all(|b| b.is_ascii_alphanumeric())
}

/// A literal contract-valid session id: `ses_` + 12 lowercase hex + 14 base62
/// (`0123456789ab` | `cdefABCDEF0123`).
fn contract_session() -> String {
    format!("ses_{}", "0123456789abcdefABCDEF0123")
}

#[test]
fn test_resolve_upstream_session_preserves_valid_zen_session() {
    // `ses_` (4) + 12 lowercase hex + 14 alphanumeric = 30 chars.
    let valid_ses = contract_session();
    assert_eq!(valid_ses.len(), 30, "fixture must be 30 chars");
    assert!(is_valid_zen_session(&valid_ses));

    let headers = headers_from(&[("x-opencode-session", &valid_ses)]);
    let resolved = resolve_upstream_session(&headers);
    assert_eq!(
        resolved, valid_ses,
        "CONTRACT C1: a contract-valid ses_<12 hex><14 alnum> session must be preserved as-is"
    );

    // Also verify when passed via alias headers (affinity / session id).
    let headers_alias = headers_from(&[("x-session-affinity", &valid_ses)]);
    let resolved_alias = resolve_upstream_session(&headers_alias);
    assert_eq!(
        resolved_alias, valid_ses,
        "CONTRACT C1: contract-valid session passed via `x-session-affinity` must also be preserved"
    );

    let headers_sid = headers_from(&[("x-session-id", &valid_ses)]);
    assert_eq!(
        resolve_upstream_session(&headers_sid),
        valid_ses,
        "CONTRACT C1: contract-valid session passed via `x-session-id` must also be preserved"
    );
}

#[test]
fn test_resolve_upstream_session_rejects_and_falls_back_on_malformed_sessions() {
    // Every rejected value: (label, downstream value). The contract requires the
    // executor to discard it and fall back to a freshly generated legal id.
    let malformed: Vec<(&str, String)> = vec![
        (
            "DSH legacy session format `session-dsh-1234`",
            "session-dsh-1234".to_string(),
        ),
        (
            "64 lowercase hex chars (v1 misread shape, upstream 403)",
            format!("ses_{}", "0".repeat(64)),
        ),
        (
            "25-char suffix (29 total)",
            format!("ses_{}", "0".repeat(25)),
        ),
        (
            "27-char suffix (31 total)",
            format!("ses_{}", "0".repeat(27)),
        ),
        (
            "`ses_` + too-short suffix",
            "ses_short123".to_string(),
        ),
        (
            // 12 hex + 14 specials = 26 payload chars, so the value is rejected
            // for its charset, not for its length.
            "`ses_` + 26 chars containing special characters",
            "ses_0123456789ab!@#$%^&*()-+=~".to_string(),
        ),
        (
            // 12 hex + 1 space + 13 alnum = 26 payload chars: rejected because of
            // the whitespace, not because of a short length.
            "`ses_` + 26 chars containing whitespace",
            "ses_0123456789ab abcdefABCDEF0".to_string(),
        ),
        (
            "`ses-` hyphen prefix",
            format!("ses-{}", "a".repeat(26)),
        ),
        (
            "UUID",
            "550e8400-e29b-41d4-a716-446655440000".to_string(),
        ),
        (
            "blank value",
            "   ".to_string(),
        ),
    ];

    for (label, malformed_value) in malformed {
        let headers = headers_from(&[("x-opencode-session", &malformed_value)]);
        let resolved = resolve_upstream_session(&headers);
        assert_ne!(
            resolved, malformed_value,
            "CONTRACT C1: non-standard session ({label}) must be discarded, not forwarded upstream; \
             got it back unchanged: {resolved:?}"
        );
        assert!(
            is_valid_zen_session(&resolved),
            "CONTRACT C1: fallback for non-standard session ({label}) must be a freshly generated \
             ^ses_[0-9a-f]{{12}}[0-9A-Za-z]{{14}}$ id, got {resolved:?}"
        );
    }
}
