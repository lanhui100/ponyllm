//! FROZEN RED-PHASE ACCEPTANCE — Wave-1b (contract v5, review-counterexample
//! driven). Additions over v4:
//!
//! - **F1**: `is_reserved_key` compares ASCII only, but Unicode simple-fold
//!   makes `ſystem` (U+017F) / `ma\u{212A}x_tokens` (U+212A KELVIN SIGN) fold
//!   to `system` / `max_tokens` on Rust's `to_lowercase()` (and on upstreams
//!   that case-fold keys). v5 rule: **any key containing a non-ASCII byte is
//!   dropped outright** — no folding, no aliasing.
//! - **A1**: the 16 KiB cap is currently measured with a full `to_vec` per
//!   field (a 32 MiB value peaks ≈64 MiB / ~150 ms). v5 measures with a
//!   bounded counting writer. Assertion (no timing, to stay non-flaky): a
//!   32 MiB single value is dropped, no panic, and the request still works.
//! - **A2**: key bytes were never counted, so an 8 MiB *key* with a 1-byte
//!   value bypassed the budget. v5: key bytes count toward the budget AND a
//!   single key is capped at 256 bytes.
//! - **Functional regression**: `previous_response_id` / `conversation` /
//!   `prompt` leave `GLOBAL_CONTROL_KEYS` (`CreateResponseRequest` has no such
//!   explicit field, so stripping them silently breaks `/v1/responses`
//!   chained conversations), while `store` / `metadata` stay reserved.

use std::collections::HashMap;

use serde_json::{json, Value};

use ponyllm_protocol::sanitize::{sanitize_extra, TargetProtocol, MAX_EXTRA_VALUE_BYTES};
use ponyllm_protocol::translator::{chat_to_anthropic_request, chat_to_responses_request};

const CHAT_MODEL: &str = "claude-3-5-sonnet-20241022";

fn extra_map(pairs: &[(String, Value)]) -> HashMap<String, Value> {
    pairs.iter().cloned().collect()
}

fn pair(key: impl Into<String>, value: Value) -> (String, Value) {
    (key.into(), value)
}

fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

// ---------------------------------------------------------------------------
// F1 — non-ASCII / Unicode-fold key aliases can never shadow a reserved field
// ---------------------------------------------------------------------------

#[test]
fn f1_unicode_fold_aliases_of_reserved_keys_are_dropped() {
    // Arrange: a Chat body whose `extra` carries four spellings of "system":
    // two ASCII case variants (already handled), one Unicode simple-fold alias
    // (U+017F LONG S) and one Unicode KELVIN SIGN alias of `max_tokens`.
    let mut body = json!({
        "model": CHAT_MODEL,
        "max_tokens": 1024,
        "messages": [
            {"role": "system", "content": "GATEWAY_SYSTEM"},
            {"role": "user", "content": "hi"}
        ]
    });
    {
        let obj = body.as_object_mut().expect("chat body object");
        obj.insert("\u{017F}ystem".to_string(), json!("FOLD_INJECT"));
        obj.insert("ma\u{212A}x_tokens".to_string(), json!(999999));
        obj.insert("sYsTeM".to_string(), json!("CASE_INJECT"));
        obj.insert("system".to_string(), json!("RAW_INJECT"));
    }
    let req: ponyllm_protocol::openai::chat::ChatCompletionRequest =
        serde_json::from_value(body).expect("chat body deserializes");

    // Act
    let raw = serde_json::to_string(&chat_to_anthropic_request(&req).expect("translation"))
        .expect("serialize upstream body");
    let body: Value = serde_json::from_str(&raw).expect("upstream body parses");

    // Assert: exactly one `system` pair, the gateway value, no alias survives.
    assert_eq!(
        occurrences(&raw, "\"system\":"),
        1,
        "F1: upstream body must carry exactly one `system` pair; body = {raw}"
    );
    assert_eq!(
        body.get("system"),
        Some(&Value::String("GATEWAY_SYSTEM".to_string())),
        "F1: `system` must be the gateway value; body = {raw}"
    );
    for leaked in ["FOLD_INJECT", "CASE_INJECT", "RAW_INJECT"] {
        assert!(
            !raw.contains(leaked),
            "F1: injected value {leaked} must never reach the upstream body; body = {raw}"
        );
    }
    assert!(
        !raw.contains('\u{017F}'),
        "F1: the U+017F `ſystem` alias key must be dropped, not forwarded; body = {raw}"
    );
    assert!(
        !raw.contains('\u{212A}'),
        "F1: the U+212A `maKx_tokens` alias key must be dropped, not forwarded; body = {raw}"
    );
    assert!(
        body.get("\u{017F}ystem").is_none() && body.get("ma\u{212A}x_tokens").is_none(),
        "F1: non-ASCII alias keys must be absent from the parsed body; body = {body}"
    );

    // Direct contract check: every non-ASCII key dies, ASCII neighbours live.
    let out = sanitize_extra(
        extra_map(&[
            pair("\u{017F}ystem", json!("fold")),
            pair("ma\u{212A}x_tokens", json!(999999)),
            pair("sYsTeM", json!("case")),
            pair("system", json!("raw")),
            pair("x_ünicode", json!("non-ascii-but-not-reserved")),
            pair("x_keep", json!("ascii-legal")),
        ]),
        TargetProtocol::OpenAiChat,
    );
    assert_eq!(
        out.len(),
        1,
        "F1: only the ASCII legal key may survive; out = {out:?}"
    );
    assert_eq!(
        out.get("x_keep"),
        Some(&json!("ascii-legal")),
        "F1: legal ASCII extension keys must stay; out = {out:?}"
    );
}

// ---------------------------------------------------------------------------
// A1 — a 32 MiB single value is dropped without panic
// ---------------------------------------------------------------------------

#[test]
fn a1_32mib_single_value_is_dropped_without_panic() {
    // Arrange: 32 MiB in one value — 2048× the 16 KiB per-value budget.
    let huge = "A".repeat(32 * 1024 * 1024);
    assert!(huge.len() > MAX_EXTRA_VALUE_BYTES);

    // Act
    let out = sanitize_extra(
        extra_map(&[pair("x_huge", json!(huge)), pair("x_keep", json!("ok"))]),
        TargetProtocol::OpenAiChat,
    );

    // Assert: the huge value is gone, the small one survived, nothing panicked.
    assert!(
        out.get("x_huge").is_none(),
        "A1: a 32 MiB value must be dropped (no unbounded copy may survive sanitization)"
    );
    assert_eq!(
        out.get("x_keep"),
        Some(&json!("ok")),
        "A1: an in-budget value must survive next to a huge one; out = {out:?}"
    );
    assert_eq!(
        out.len(),
        1,
        "A1: exactly one entry survives; out = {out:?}"
    );
}

// ---------------------------------------------------------------------------
// A2 — key bytes are budgeted, and a single key is capped at 256 bytes
// ---------------------------------------------------------------------------

#[test]
fn a2_overlong_keys_are_dropped_and_key_bytes_count_against_the_budget() {
    // (a) single-key length cap: 256 bytes kept, 257 dropped.
    let key_256 = "k".repeat(256);
    let key_257 = "k".repeat(257);
    let out = sanitize_extra(
        extra_map(&[
            pair(key_256.clone(), json!(1)),
            pair(key_257.clone(), json!(2)),
        ]),
        TargetProtocol::OpenAiChat,
    );
    assert_eq!(
        out.len(),
        1,
        "A2: only the 256-byte key may survive; out keys = {:?}",
        out.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        out.get(&key_256),
        Some(&json!(1)),
        "A2: a 256-byte key is within the per-key cap"
    );
    assert!(
        out.get(&key_257).is_none(),
        "A2: a 257-byte key exceeds the per-key cap and must be dropped"
    );

    // (b) an 8 MiB key with a 1-byte value cannot bypass the budget.
    let big_key = "z".repeat(8 * 1024 * 1024);
    let out = sanitize_extra(
        extra_map(&[pair(big_key, json!(1))]),
        TargetProtocol::OpenAiChat,
    );
    assert!(
        out.is_empty(),
        "A2: an 8 MiB key (value = 1 byte) must not bypass the size budget; out keys = {:?}",
        out.keys().collect::<Vec<_>>()
    );

    // (c) key bytes count toward the 16 KiB budget:
    //     key(200) + value(15000) ≈ 15.2 KiB → kept;
    //     key(200) + value(16350) ≈ 16.6 KiB → dropped, although the VALUE
    //     alone would be under the per-value cap.
    let key_200 = "a".repeat(200);
    let kept = sanitize_extra(
        extra_map(&[pair(key_200.clone(), json!("b".repeat(15000)))]),
        TargetProtocol::OpenAiChat,
    );
    assert_eq!(
        kept.len(),
        1,
        "A2: key(200)+value(15000) is inside the 16 KiB budget and must be kept"
    );
    let dropped = sanitize_extra(
        extra_map(&[pair(key_200, json!("c".repeat(16350)))]),
        TargetProtocol::OpenAiChat,
    );
    assert!(
        dropped.is_empty(),
        "A2: key bytes must count toward the budget — key(200)+value(16350) exceeds 16 KiB and must be dropped; out keys = {:?}",
        dropped.keys().collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Functional regression — responses chaining keys are no longer stripped
// ---------------------------------------------------------------------------

#[test]
fn regression_previous_response_id_is_preserved_for_responses_target() {
    // Direct contract check.
    let out = sanitize_extra(
        extra_map(&[
            pair("previous_response_id", json!("resp_prev_123")),
            pair("conversation", json!("conv_123")),
            pair("prompt", json!({"id": "pmpt_123"})),
            pair("store", json!(true)),
            pair("metadata", json!({"user_id": "ATTACKER_META"})),
            pair("x_keep", json!(1)),
        ]),
        TargetProtocol::OpenAiResponses,
    );
    assert_eq!(
        out.get("previous_response_id"),
        Some(&json!("resp_prev_123")),
        "v5: `previous_response_id` must pass through to the Responses target (chained conversations); out = {out:?}"
    );
    assert_eq!(
        out.get("conversation"),
        Some(&json!("conv_123")),
        "v5: `conversation` must pass through to the Responses target; out = {out:?}"
    );
    assert_eq!(
        out.get("prompt"),
        Some(&json!({"id": "pmpt_123"})),
        "v5: `prompt` must pass through to the Responses target; out = {out:?}"
    );
    assert!(
        out.get("store").is_none(),
        "v5: `store` stays reserved for the Responses target; out = {out:?}"
    );
    assert!(
        out.get("metadata").is_none(),
        "v5: `metadata` stays reserved for the Responses target (the explicit field carries it); out = {out:?}"
    );

    // End-to-end through the real translator.
    let mut body = json!({
        "model": "gpt-4o-mini",
        "max_tokens": 256,
        "messages": [{"role": "user", "content": "hi"}]
    });
    {
        let obj = body.as_object_mut().expect("chat body object");
        obj.insert("previous_response_id".to_string(), json!("resp_prev_456"));
        obj.insert("store".to_string(), json!(true));
    }
    let req: ponyllm_protocol::openai::chat::ChatCompletionRequest =
        serde_json::from_value(body).expect("chat body deserializes");
    let raw = serde_json::to_string(&chat_to_responses_request(&req).expect("translation"))
        .expect("serialize responses body");
    let wire: Value = serde_json::from_str(&raw).expect("responses body parses");

    assert_eq!(
        wire.get("previous_response_id"),
        Some(&json!("resp_prev_456")),
        "v5: the chained-conversation key must survive translation to the Responses wire; body = {raw}"
    );
    assert!(
        wire.get("store").is_none(),
        "v5: `store` must not reach the Responses wire; body = {raw}"
    );
}
