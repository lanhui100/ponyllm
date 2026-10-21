//! Frozen acceptance tests — OpenCode Zen session header contract (hex12 form).
//!
//! Machine-verified upstream contract (do not re-litigate):
//!
//! * `^ses_[0-9a-f]{12}[0-9A-Za-z]{14}$` → upstream HTTP 200
//!   (`ses_5e67a59ef81ehxVk8zG0o0Nd9h`).
//! * `ses_GQs9Ss1p7NsR7NoY1p7hRVA5sV` (first 12 payload chars are not all
//!   lowercase hex) → upstream HTTP 403 `FreeTierError`.
//!
//! Total length is therefore always 3 + 1 + 12 + 14 = 30 chars, of which the
//! first 12 payload chars MUST be lowercase `[0-9a-f]` and the trailing 14
//! chars may be any `[0-9A-Za-z]`.
//!
//! SUT under test (`ponyllm_core::executor`):
//!   * `new_upstream_session_id() -> String`
//!   * `is_valid_zen_session_format(&str) -> bool`
//!
//! Discipline: real assertions only (no `#[ignore]`, no empty bodies, no
//! assertions that cannot fail). Every failure message carries the actual
//! offending value so a red run is diagnosable from stdout alone.

use ponyllm_core::executor::{is_valid_zen_session_format, new_upstream_session_id};
use std::collections::HashSet;

/// Number of generated ids per generative test. 200 is large enough to make a
/// "sometimes emits a non-hex prefix" implementation fail with overwhelming
/// probability, small enough to keep the suite fast.
const GENERATION_SAMPLES: usize = 200;

/// Reference implementation of the frozen regex
/// `^ses_[0-9a-f]{12}[0-9A-Za-z]{14}$` — hand-rolled because `regex` is not a
/// dependency of this crate and adding one for a single 30-char shape is not
/// justified. Anchored on both ends: no prefix/suffix slack is allowed.
fn matches_frozen_contract(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("ses_") else {
        return false;
    };
    let bytes = rest.as_bytes();
    if bytes.len() != 26 {
        return false;
    }
    let (head, tail) = bytes.split_at(12);
    head.iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        && tail.iter().all(|b| b.is_ascii_alphanumeric())
}

/// Asserts that `value` is accepted by the SUT *and* by the reference matcher,
/// i.e. both agree it is a legal upstream session value.
#[track_caller]
fn assert_accepted(value: &str, case: &str) {
    assert!(
        matches_frozen_contract(value),
        "TEST BUG[{case}]: fixture {value:?} does not satisfy the frozen regex \
         ^ses_[0-9a-f]{{12}}[0-9A-Za-z]{{14}}$"
    );
    assert!(
        is_valid_zen_session_format(value),
        "CONTRACT hex12[{case}]: is_valid_zen_session_format must ACCEPT {value:?} \
         (len {}, matches ^ses_[0-9a-f]{{12}}[0-9A-Za-z]{{14}}$)",
        value.chars().count()
    );
}

/// Asserts the SUT rejects `value`, with the actual value in the message.
#[track_caller]
fn assert_rejected(value: &str, case: &str) {
    assert!(
        !is_valid_zen_session_format(value),
        "CONTRACT hex12[{case}]: is_valid_zen_session_format must REJECT {value:?} \
         (len {}, first 12 payload chars {:?})",
        value.chars().count(),
        value
            .strip_prefix("ses_")
            .map(|r| r.chars().take(12).collect::<String>())
            .unwrap_or_default()
    );
}

// ---------------------------------------------------------------------------
// Generator: new_upstream_session_id()
// ---------------------------------------------------------------------------

/// G-1: every freshly generated id matches the frozen 12-hex + 14-base62 shape,
/// is 30 chars long, and exposes a lowercase-hex 12-char prefix.
#[test]
fn g1_generated_session_id_matches_hex12_contract() {
    // Arrange / Act / Assert are interleaved deliberately: each sample is an
    // independent Act, and the loop keeps the first offending value visible.
    for i in 0..GENERATION_SAMPLES {
        let id = new_upstream_session_id();

        // (a) total length fixates the shape for the wire gate.
        assert_eq!(
            id.chars().count(),
            30,
            "CONTRACT hex12[G-1 sample {i}]: generated session id must be 30 chars \
             (4 + 12 hex + 14 base62), got {} for {id:?}",
            id.chars().count()
        );

        // (b) mandatory `ses_` prefix.
        let payload = id.strip_prefix("ses_").unwrap_or_else(|| {
            panic!("CONTRACT hex12[G-1 sample {i}]: generated session id must start with `ses_`, got {id:?}")
        });

        // (c) payload is exactly 26 chars.
        assert_eq!(
            payload.chars().count(),
            26,
            "CONTRACT hex12[G-1 sample {i}]: payload after `ses_` must be 26 chars, \
             got {} for {id:?}",
            payload.chars().count()
        );

        // (d) the first 12 payload chars are lowercase hex — the exact bit the
        // upstream free-tier gate checks (non-hex → 403 FreeTierError).
        let head: String = payload.chars().take(12).collect();
        assert_eq!(
            head.chars().count(),
            12,
            "CONTRACT hex12[G-1 sample {i}]: hex head must be 12 chars, got {head:?} from {id:?}"
        );
        assert!(
            head.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "CONTRACT hex12[G-1 sample {i}]: first 12 payload chars must be lowercase hex \
             [0-9a-f], got {head:?} in {id:?}"
        );

        // (e) the trailing 14 chars are unrestricted base62.
        let tail: String = payload.chars().skip(12).collect();
        assert_eq!(
            tail.chars().count(),
            14,
            "CONTRACT hex12[G-1 sample {i}]: trailing segment must be 14 base62 chars, \
             got {tail:?} from {id:?}"
        );
        assert!(
            tail.chars().all(|c| c.is_ascii_alphanumeric()),
            "CONTRACT hex12[G-1 sample {i}]: trailing segment must be [0-9A-Za-z], \
             got {tail:?} in {id:?}"
        );

        // (f) full-string agreement with the frozen regex.
        assert!(
            matches_frozen_contract(&id),
            "CONTRACT hex12[G-1 sample {i}]: generated id {id:?} must match \
             ^ses_[0-9a-f]{{12}}[0-9A-Za-z]{{14}}$"
        );
    }
}

/// G-2: the generator and the validator agree — every generated id is accepted
/// by `is_valid_zen_session_format`, and 200 samples are pairwise distinct
/// (a degenerate/constant generator would otherwise slip through G-1 when its
/// single output happens to be well-formed).
#[test]
fn g2_generated_ids_are_valid_and_distinct() {
    let mut seen: HashSet<String> = HashSet::new();
    for i in 0..GENERATION_SAMPLES {
        let id = new_upstream_session_id();
        assert!(
            is_valid_zen_session_format(&id),
            "CONTRACT hex12[G-2 sample {i}]: validator must ACCEPT the generator's own \
             output, rejected {id:?}"
        );
        assert!(
            seen.insert(id.clone()),
            "CONTRACT hex12[G-2 sample {i}]: generated session ids must be distinct \
             (no constant/cyclic generator), got duplicate {id:?}"
        );
    }
    assert_eq!(
        seen.len(),
        GENERATION_SAMPLES,
        "CONTRACT hex12[G-2]: expected {GENERATION_SAMPLES} distinct ids, got {}",
        seen.len()
    );
}

// ---------------------------------------------------------------------------
// Validator: accepted forms
// ---------------------------------------------------------------------------

/// V-ACCEPT-1: the machine-verified live sample that returned HTTP 200.
#[test]
fn v_accept_live_upstream_sample() {
    assert_accepted("ses_5e67a59ef81ehxVk8zG0o0Nd9h", "live-200 sample");
}

/// V-ACCEPT-2: upper bound of the charset — 12 hex + 14 uppercase `A`.
#[test]
fn v_accept_hex12_plus_fourteen_uppercase() {
    assert_accepted("ses_0123456789abAAAAAAAAAAAAAA", "hex12 + 14 'A'");
}

/// V-ACCEPT-3: lower bound of the charset — 12 hex + 14 lowercase letters.
#[test]
fn v_accept_hex12_plus_fourteen_lowercase() {
    assert_accepted("ses_0123456789ababcdefghijklmn", "hex12 + 14 lowercase");
}

/// V-ACCEPT-4: boundary digit values of the hex head (all `0`, all `9`, all
/// `f`) with a numeric tail.
#[test]
fn v_accept_hex_boundary_digits() {
    assert_accepted("ses_00000000000000000000000000", "hex head all '0'");
    assert_accepted("ses_ffffffffffff00000000000000", "hex head all 'f'");
    assert_accepted("ses_9999999999990123456789ABCD", "hex head all '9'");
}

/// V-ACCEPT-5: trailing U+0000-free mixed base62 tail must not trip the head
/// rule — regression guard against an implementation that validates the whole
/// payload as hex (which would reject the 200-documented sample).
#[test]
fn v_accept_tail_may_be_any_base62() {
    assert_accepted("ses_a1b2c3d4e5f6ZzYyXxWwVvUuTt", "mixed-case tail");
}

// ---------------------------------------------------------------------------
// Validator: rejected forms
// ---------------------------------------------------------------------------

/// V-REJECT-1: the machine-verified upstream 403 `FreeTierError` sample — 26
/// base62 chars whose first 12 are not all lowercase hex.
#[test]
fn v_reject_live_upstream_403_sample() {
    assert_rejected("ses_GQs9Ss1p7NsR7NoY1p7hRVA5sV", "live-403 FreeTierError sample");
}

/// V-REJECT-2: the pre-contract wire shape emitted by the current generator —
/// 26 base62 chars with a non-hex prefix. This is the shape the upstream gate
/// rejects, so the validator must not bless it.
#[test]
fn v_reject_26_base62_with_non_hex_prefix() {
    assert_rejected("ses_ZZZZZZZZZZZZ0123456789abcd", "12 non-hex chars + 14 base62");
    assert_rejected("ses_xxxxxxxxxxxxabcdefghijklmn", "12 'x' + 14 lowercase");
    assert_rejected("ses_9876543210zzABCDEFGHIJKLMN", "one trailing 'z' in head");
}

/// V-REJECT-3: uppercase hex letters in the 12-char head are NOT valid hex —
/// the contract requires `[0-9a-f]`, so `A`–`F` must be rejected in the head.
#[test]
fn v_reject_uppercase_hex_in_head() {
    assert_rejected("ses_A23456789012hxVk8zG0o0Nd9h", "leading 'A' in head");
    assert_rejected("ses_5e67a59ef81EhxVk8zG0o0Nd9h", "trailing 'E' in head");
    assert_rejected("ses_ABCDEF012345hxVk8zG0o0Nd9h", "all-uppercase hex head");
}

/// V-REJECT-4: `ses_` + 64 hex (the v1 misreading) and other over-long shapes.
#[test]
fn v_reject_wrong_lengths() {
    let hex64 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let long = format!("ses_{hex64}");
    assert_rejected(&long, "ses_ + 64 hex");

    // Payload-length variants around the frozen 26-char payload.
    assert_rejected("ses_0123456789ab0123456789abc", "payload 25 chars");
    assert_rejected("ses_0123456789ab0123456789abcde", "payload 27 chars");
    assert_rejected("ses_0123456789ab", "payload 12 chars (head only)");
    assert_rejected("ses_", "payload 0 chars");
}

/// V-REJECT-5: missing / altered prefix.
#[test]
fn v_reject_missing_or_altered_prefix() {
    assert_rejected("5e67a59ef81ehxVk8zG0o0Nd9h", "no prefix at all");
    assert_rejected("ses-5e67a59ef81ehxVk8zG0o0Nd9h", "hyphen separator");
    assert_rejected("SES_5e67a59ef81ehxVk8zG0o0Nd9h", "uppercase prefix");
    assert_rejected("ses5e67a59ef81ehxVk8zG0o0Nd9h", "missing underscore");
    assert_rejected("xses_5e67a59ef81ehxVk8zG0o0Nd9h", "extra leading char");
    assert_rejected("ses_5e67a59ef81ehxVk8zG0o0Nd9hh", "extra trailing char");
}

/// V-REJECT-6: empty and whitespace-bearing values (the validator must not
/// trim — trimming is the caller's job and a trimmed value that then matches
/// would let raw header garbage through).
#[test]
fn v_reject_empty_and_whitespace() {
    assert_rejected("", "empty string");
    assert_rejected(" ", "single space");
    assert_rejected("  ses_5e67a59ef81ehxVk8zG0o0Nd9h", "leading space");
    assert_rejected("ses_5e67a59ef81ehxVk8zG0o0Nd9h ", "trailing space");
    assert_rejected("ses_5e67a59ef81ehxVk8zG0o0Nd9h\n", "trailing newline");
    assert_rejected("ses_5e67a59ef81e hxVk8zG0o0Nd9", "embedded space");
    assert_rejected("\tses_5e67a59ef81ehxVk8zG0o0Nd9h", "leading tab");
}

/// V-REJECT-7: non-ASCII / control-character injection. All three fixtures are
/// exactly 26 *chars* after `ses_` (the é fixture is 27 *bytes*), so length
/// arithmetic alone cannot reject them — only a byte-level charset check can.
#[test]
fn v_reject_non_ascii_and_control_chars() {
    assert_rejected(
        "ses_5e67a59ef81eéhxVk8zG0o0Nd9",
        "non-ASCII 'é' (26 chars / 27 bytes payload)",
    );
    assert_rejected("ses_5e67a59ef81ehxVk8zG0o0Nd9\u{0000}", "trailing NUL");
    assert_rejected("ses_5e67a59ef81ehxVk8zG0o0Nd9\u{1F600}", "emoji in payload");
}
