//! Red-phase acceptance tests — CONTRACT-v1 C1 (session id format), v1.1 revision.
//!
//! Frozen contract: `/home/dm/ponyllm/.dev-team/zen-rc/CONTRACT-v1.md` (read-only).
//!
//! C1 v1.1 (empirically verified 2026-10-20 on a healthy tencent exit): the ONLY
//! hard gate is `ses_` + exactly 26 alphanumeric chars (`^ses_[A-Za-z0-9]{26}$`,
//! 30 chars total). A `ses_` + 64 lowercase hex shape — the v1 misreading — is
//! rejected by the upstream gate (HTTP 403), so `is_valid_zen_session_format()`
//! MUST reject it and downstream values in that shape are replaced by a freshly
//! generated compliant id.
//!
//! Red-phase discipline: real assertions only, no `#[ignore]`, no empty bodies.

use ponyllm_core::executor::{is_valid_zen_session_format, new_upstream_session_id};
use std::collections::HashSet;

/// Contract C1 v1.1 shape: `^ses_[A-Za-z0-9]{26}$` (30 chars total).
fn is_contract_session(s: &str) -> bool {
    match s.strip_prefix("ses_") {
        Some(rest) => {
            rest.len() == 26 && rest.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

/// C1-1: `new_upstream_session_id()` returns `ses_` + exactly 26 alphanumeric
/// chars (30 total), and never repeats itself.
#[test]
fn c1_new_upstream_session_id_is_ses_plus_26_alnum() {
    let mut seen: HashSet<String> = HashSet::new();
    for _ in 0..64 {
        let id = new_upstream_session_id();
        let suffix = id.strip_prefix("ses_").unwrap_or_else(|| {
            panic!(
                "CONTRACT C1: new_upstream_session_id() must start with `ses_`, got {id:?}"
            )
        });
        assert_eq!(
            suffix.len(),
            26,
            "CONTRACT C1: suffix must be exactly 26 alphanumeric chars (30 total), got {id:?} (suffix len {})",
            suffix.len()
        );
        assert!(
            suffix.chars().all(|c| c.is_ascii_alphanumeric()),
            "CONTRACT C1: suffix must be alphanumeric [0-9A-Za-z] only, got {id:?}"
        );
        assert!(
            is_contract_session(&id),
            "CONTRACT C1: generated id must match ^ses_[A-Za-z0-9]{{26}}$, got {id:?}"
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
        "CONTRACT C1: must accept 26 alphanumeric chars (0..9 boundary), got {all_digits:?}"
    );

    let mixed = format!("ses_{}", "0123456789abcdefghijklmnop"); // 10 digits + 16 letters = 26
    assert_eq!(mixed.len(), 30, "fixture must be 30 chars");
    assert!(
        is_valid_zen_session_format(&mixed),
        "CONTRACT C1: must accept 26 mixed alphanumeric chars, got {mixed:?}"
    );

    let mixed_case = format!("ses_{}", "AbCdEf0123456789ZzYyXxWwVv"); // 26 mixed-case alnum
    assert_eq!(mixed_case.len(), 30, "fixture must be 30 chars");
    assert!(
        is_valid_zen_session_format(&mixed_case),
        "CONTRACT C1: must accept mixed-case alphanumeric suffix, got {mixed_case:?}"
    );
}

/// C1-3: every non-contract form is rejected (64-hex / 25 / 27 chars / uppercase
/// hex / `ses-` / empty / whitespace / UUID / wrong length).
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
            "uppercase-hex-only suffix is alphanumeric and MUST be accepted, so we use a non-alnum instead",
            format!("ses_{}", "ABCDEF0123456789ABCDEF0123-"), // 26 alnum + trailing '-' → rejected
        ),
        (
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
             the ONLY accepted shape is ^ses_[A-Za-z0-9]{{26}}$"
        );
    }
}
