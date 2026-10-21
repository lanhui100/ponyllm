//! Red-phase acceptance tests — CONTRACT-v1 C1 (session id format), v1.2 revision.
//!
//! Frozen contract: `/home/dm/ponyllm/.dev-team/zen-rc/CONTRACT-v1.md` (read-only).
//!
//! C1 v1.2 (machine-verified 2026-10-21, same payload / IP / key): the gate does
//! NOT accept an arbitrary 26-char alphanumeric suffix. The accepted shape is
//! `^ses_[0-9a-f]{12}[0-9A-Za-z]{14}$` — 12 lowercase hex chars (the real client
//! puts a millisecond clock plus a counter there) followed by 14 base62 chars.
//! Receipts: `ses_5e67a59ef81ehxVk8zG0o0Nd9h` → HTTP 200,
//! `ses_4c09de94431dAAAAAAAAAAAAAA` → HTTP 200, but 26-char all-base62 ids with
//! a non-hex leading char (`ses_GQs9Ss1p7NsR7NoY1p7hRVA5sV`,
//! `ses_PziUUHVrMVc4TPCkQnvULUGYLk`, ...) → HTTP 403 FreeTierError. The old
//! `ses_` + 64 lowercase hex shape is likewise rejected (HTTP 403), so
//! `is_valid_zen_session_format()` MUST reject both and downstream values in
//! those shapes are replaced by a freshly generated compliant id.
//!
//! Red-phase discipline: real assertions only, no `#[ignore]`, no empty bodies.

use ponyllm_core::executor::{is_valid_zen_session_format, new_upstream_session_id};
use std::collections::HashSet;

/// Contract C1 v1.2 shape: `^ses_[0-9a-f]{12}[0-9A-Za-z]{14}$` (30 chars total).
fn is_contract_session(s: &str) -> bool {
    match s.strip_prefix("ses_") {
        Some(rest) => {
            let bytes = rest.as_bytes();
            bytes.len() == 26
                && bytes[..12]
                    .iter()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
                && bytes[12..].iter().all(|b| b.is_ascii_alphanumeric())
        }
        None => false,
    }
}

/// C1-1: `new_upstream_session_id()` returns `ses_` + 12 lowercase hex + 14
/// alphanumeric chars (30 total), and never repeats itself.
#[test]
fn c1_new_upstream_session_id_is_ses_plus_hex12_and_base62() {
    let mut seen: HashSet<String> = HashSet::new();
    for _ in 0..64 {
        let id = new_upstream_session_id();
        let suffix = id.strip_prefix("ses_").unwrap_or_else(|| {
            panic!("CONTRACT C1: new_upstream_session_id() must start with `ses_`, got {id:?}")
        });
        assert_eq!(
            suffix.len(),
            26,
            "CONTRACT C1: suffix must be exactly 26 chars (12 hex + 14 base62, 30 total), got {id:?} (suffix len {})",
            suffix.len()
        );
        assert!(
            suffix.chars().all(|c| c.is_ascii_alphanumeric()),
            "CONTRACT C1: suffix must be alphanumeric [0-9A-Za-z] only, got {id:?}"
        );
        assert!(
            suffix
                .bytes()
                .take(12)
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f')),
            "CONTRACT C1: first 12 suffix chars must be lowercase hex [0-9a-f], got {id:?}"
        );
        assert!(
            is_contract_session(&id),
            "CONTRACT C1: generated id must match ^ses_[0-9a-f]{{12}}[0-9A-Za-z]{{14}}$, got {id:?}"
        );
        assert!(
            seen.insert(id.clone()),
            "CONTRACT C1: generated session ids must be unique, got duplicate {id:?}"
        );
    }
}

/// C1-2: `is_valid_zen_session_format` accepts the contract form (generated and
/// literal boundary samples).
#[test]
fn c1_is_valid_zen_session_format_accepts_contract_form() {
    let generated = new_upstream_session_id();
    assert!(
        is_valid_zen_session_format(&generated),
        "CONTRACT C1: is_valid_zen_session_format must accept a freshly generated id, got {generated:?}"
    );

    let all_digits = format!("ses_{}", "0".repeat(26));
    assert!(
        is_valid_zen_session_format(&all_digits),
        "CONTRACT C1: must accept 26 alnum chars with an all-hex prefix (0..9 boundary), got {all_digits:?}"
    );

    let mixed = format!("ses_{}", "0123456789abcdefghijklmnop"); // 12 hex + 14 base62
    assert_eq!(mixed.len(), 30, "fixture must be 30 chars");
    assert!(
        is_valid_zen_session_format(&mixed),
        "CONTRACT C1: must accept 12 hex + 14 mixed alphanumeric chars, got {mixed:?}"
    );

    let mixed_case = format!("ses_{}", "a1b2c3d4e5f6AbCdEfZzYyXxWw"); // hex12 + mixed-case 14
    assert_eq!(mixed_case.len(), 30, "fixture must be 30 chars");
    assert!(
        is_valid_zen_session_format(&mixed_case),
        "CONTRACT C1: must accept a mixed-case alphanumeric suffix after the hex prefix, got {mixed_case:?}"
    );

    let upper_suffix = format!("ses_{}", "abcdef012345ABCDEF01234567"); // hex12 + uppercase base62
    assert_eq!(upper_suffix.len(), 30, "fixture must be 30 chars");
    assert!(
        is_valid_zen_session_format(&upper_suffix),
        "CONTRACT C1: uppercase chars are legal after the hex prefix, got {upper_suffix:?}"
    );

    // Observed HTTP 200 upstream: hex12 prefix + 14 literal 'A' suffix.
    let observed_ok = "ses_4c09de94431dAAAAAAAAAAAAAA";
    assert_eq!(observed_ok.len(), 30, "fixture must be 30 chars");
    assert!(
        is_valid_zen_session_format(observed_ok),
        "CONTRACT C1: hex12 + 14 'A' is accepted upstream (observed 200), got {observed_ok:?}"
    );
}

/// C1-3: every non-contract form is rejected (64-hex / 25 / 27 chars / non-hex
/// 12-prefix / uppercase hex prefix / `ses-` / empty / whitespace / UUID / wrong
/// length).
#[test]
fn c1_is_valid_zen_session_format_rejects_non_contract_forms() {
    let negatives: Vec<(&str, String)> = vec![
        (
            "64 lowercase hex chars (v1 misread shape, empirically 403 upstream)",
            format!("ses_{}", "0".repeat(64)),
        ),
        (
            "25-char suffix (29 chars total)",
            format!("ses_{}", "0".repeat(25)),
        ),
        (
            "27-char suffix (31 chars total)",
            format!("ses_{}", "0".repeat(27)),
        ),
        (
            "32-char suffix (36 chars total)",
            format!("ses_{}", "0".repeat(32)),
        ),
        (
            "28-char total (`ses_` + 24 chars)",
            format!("ses_{}", "0".repeat(24)),
        ),
        (
            "26-char all-base62 suffix whose first 12 chars are not all hex (observed 403 upstream)",
            "ses_GQs9Ss1p7NsR7NoY1p7hRVA5sV".to_string(),
        ),
        (
            "26-char all-base62 suffix, non-hex leading char (observed 403 upstream)",
            "ses_PziUUHVrMVc4TPCkQnvULUGYLk".to_string(),
        ),
        (
            "26-char all-base62 suffix with `ses_`-style hex-free prefix (observed 403 upstream)",
            "ses_f5UiF7wZNEXb7qX7ffel4GaRb2".to_string(),
        ),
        (
            "uppercase hex in the 12-char prefix",
            format!("ses_{}", "0123456789ABAAAAAAAAAAAAAA"),
        ),
        (
            "non-hex base62 char inside the 12-char prefix",
            format!("ses_{}", "0123456789azABCDEF01234567"),
        ),
        (
            "hex prefix but a non-alnum char in the 14-char tail",
            format!("ses_{}", "0123456789abABCDEF0123456-"),
        ),        (
            "`ses-` hyphen prefix (not `ses_`)",
            format!("ses-{}", "a".repeat(26)),
        ),
        ("empty string", String::new()),
        ("whitespace only", "   ".to_string()),
        (
            "embedded space in suffix",
            format!("ses_{} {}", "a".repeat(24), "a"),
        ),
        (
            "UUID",
            "550e8400-e29b-41d4-a716-446655440000".to_string(),
        ),
        (
            "26 chars without the `ses_` prefix",
            "a".repeat(26),
        ),
        (
            "trailing newline after 26 alnum chars",
            format!("ses_{}\n", "a".repeat(26)),
        ),
        (
            "lowercase-hex 64 with `ses_` prefix and trailing dash",
            format!("ses_{}-", "a".repeat(26)),
        ),
    ];

    for (name, value) in negatives {
        assert!(
            !is_valid_zen_session_format(&value),
            "CONTRACT C1: is_valid_zen_session_format must reject {name}, got true for {value:?}; \
             the ONLY accepted shape is ^ses_[0-9a-f]{{12}}[0-9A-Za-z]{{14}}$"
        );
    }
}
