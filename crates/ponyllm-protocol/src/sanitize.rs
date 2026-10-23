//! Cross-protocol `extra` sanitization boundary (P0 safety fix).
//!
//! Request bodies are deserialized with `#[serde(flatten)] extra`, which accepts
//! arbitrary caller-controlled JSON keys. Forwarding that map verbatim into a
//! *different* protocol lets a caller inject fields owned by the target wire
//! format, and even inside the *same* protocol the flattened map is serialized
//! after the typed fields (`serde_json::to_value` keeps the last occurrence), so
//! a caller can overwrite gateway-computed values such as `system`,
//! `max_output_tokens` or `thinking`. This module is the single choke point that
//! decides which names and which sizes may cross that boundary.
//!
//! Contract (frozen, v4):
//! - `reserved_keys(target)` = the target's own explicit request fields
//!   ∪ [`GLOBAL_CONTROL_KEYS`], compared case-insensitively;
//! - [`sanitize_extra`] drops reserved names, caps the map at
//!   [`MAX_EXTRA_ENTRIES`] entries and drops any single value whose serialized
//!   form exceeds [`MAX_EXTRA_VALUE_BYTES`]; every other entry passes through
//!   byte-for-byte.

use std::collections::HashMap;

use serde_json::Value;

/// Downstream wire protocol a sanitized `extra` map is destined for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetProtocol {
    /// OpenAI-compatible `/v1/chat/completions`.
    OpenAiChat,
    /// OpenAI-compatible `/v1/responses`.
    OpenAiResponses,
    /// Anthropic Messages API.
    Anthropic,
}

/// Control keys that are never allowed to cross ANY translation boundary.
///
/// These are the fields the gateway itself computes (prompt/system assembly,
/// output-token clamping, thinking shape, storage pinning, conversation state)
/// plus the fields that would otherwise let a caller re-target the request at
/// server-side provider state. A client-supplied value for any of them must
/// never reach an upstream body, whichever protocol the body is written in.
pub const GLOBAL_CONTROL_KEYS: &[&str] = &[
    "system",
    "instructions",
    "thinking",
    "metadata",
    "store",
    "max_output_tokens",
    "output_config",
    "stop_sequences",
    "reasoning",
    "reasoning_effort",
    "previous_response_id",
    "conversation",
    "prompt",
    "safety_identifier",
    "stream_options",
];

/// Reserved names for the OpenAI Chat wire format.
///
/// Chat's explicit request fields followed by the [`GLOBAL_CONTROL_KEYS`] it
/// does not already own — i.e. the full v4 union, so `extra` can never
/// re-inject a typed field nor a global control key.
pub const RESERVED_CHAT: &[&str] = &[
    // ChatCompletionRequest fields.
    "model",
    "messages",
    "temperature",
    "top_p",
    "n",
    "stream",
    "stream_options",
    "stop",
    "max_tokens",
    "max_completion_tokens",
    "presence_penalty",
    "frequency_penalty",
    "logit_bias",
    "user",
    "response_format",
    "seed",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "reasoning_effort",
    // GLOBAL_CONTROL_KEYS not already covered above.
    "system",
    "instructions",
    "thinking",
    "metadata",
    "store",
    "max_output_tokens",
    "output_config",
    "stop_sequences",
    "reasoning",
    "previous_response_id",
    "conversation",
    "prompt",
    "safety_identifier",
];

/// Reserved names for the OpenAI Responses wire format (see [`RESERVED_CHAT`]).
pub const RESERVED_RESPONSES: &[&str] = &[
    // CreateResponseRequest fields.
    "model",
    "input",
    "instructions",
    "modalities",
    "tools",
    "tool_choice",
    "temperature",
    "top_p",
    "max_output_tokens",
    "stream",
    "metadata",
    "reasoning",
    "reasoning_effort",
    // GLOBAL_CONTROL_KEYS not already covered above.
    "system",
    "thinking",
    "store",
    "output_config",
    "stop_sequences",
    "previous_response_id",
    "conversation",
    "prompt",
    "safety_identifier",
    "stream_options",
];

/// Reserved names for the Anthropic Messages wire format (see [`RESERVED_CHAT`]).
pub const RESERVED_ANTHROPIC: &[&str] = &[
    // MessageRequest fields.
    "model",
    "messages",
    "max_tokens",
    "system",
    "metadata",
    "stop_sequences",
    "stream",
    "temperature",
    "top_p",
    "top_k",
    "tools",
    "tool_choice",
    "thinking",
    "output_config",
    "reasoning_effort",
    // GLOBAL_CONTROL_KEYS not already covered above.
    "instructions",
    "store",
    "max_output_tokens",
    "reasoning",
    "previous_response_id",
    "conversation",
    "prompt",
    "safety_identifier",
    "stream_options",
];

/// Hard cap on caller-supplied `extra` entries that may reach upstream.
pub const MAX_EXTRA_ENTRIES: usize = 64;

/// Hard cap on a single `extra` value's serialized size, in bytes.
pub const MAX_EXTRA_VALUE_BYTES: usize = 16 * 1024;

/// Reserved field names owned by `target`'s wire format.
///
/// Returns the target's explicit request fields unioned with
/// [`GLOBAL_CONTROL_KEYS`]; matching is case-insensitive, so callers must
/// compare with [`str::eq_ignore_ascii_case`] (as [`sanitize_extra`] does).
pub fn reserved_keys(target: TargetProtocol) -> &'static [&'static str] {
    match target {
        TargetProtocol::OpenAiChat => RESERVED_CHAT,
        TargetProtocol::OpenAiResponses => RESERVED_RESPONSES,
        TargetProtocol::Anthropic => RESERVED_ANTHROPIC,
    }
}

/// Whether `key` is a reserved name for `target` (case-insensitive).
pub fn is_reserved_key(key: &str, target: TargetProtocol) -> bool {
    reserved_keys(target)
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(key))
}

/// Serialized byte length of a single `extra` value; `None` when it cannot be
/// serialized (defensive: an unserializable value must be dropped, not panic).
fn serialized_len(value: &Value) -> Option<usize> {
    serde_json::to_vec(value).ok().map(|bytes| bytes.len())
}

/// Filters a flattened `extra` map down to the names `target` tolerates.
///
/// Rules (all fail-closed, none panicking):
/// - names reserved by `target` (case-insensitive) are dropped;
/// - an entry whose value serializes to more than [`MAX_EXTRA_VALUE_BYTES`] is
///   dropped, so one hostile payload cannot bloat the upstream request;
/// - at most [`MAX_EXTRA_ENTRIES`] entries survive — the retained subset is
///   chosen deterministically by key order, so the same input always yields the
///   same output (idempotent and reproducible under retries/replay);
/// - everything else is forwarded verbatim.
pub fn sanitize_extra(
    extra: HashMap<String, Value>,
    target: TargetProtocol,
) -> HashMap<String, Value> {
    let mut kept: Vec<(String, Value)> = extra
        .into_iter()
        .filter(|(key, _)| !is_reserved_key(key, target))
        .filter(|(_, value)| serialized_len(value).is_some_and(|len| len <= MAX_EXTRA_VALUE_BYTES))
        .collect();
    if kept.len() > MAX_EXTRA_ENTRIES {
        kept.sort_by(|left, right| left.0.cmp(&right.0));
        kept.truncate(MAX_EXTRA_ENTRIES);
    }
    kept.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn every_target_reserves_its_wire_fields_and_the_global_control_keys() {
        for (target, label) in [
            (TargetProtocol::OpenAiChat, "chat"),
            (TargetProtocol::OpenAiResponses, "responses"),
            (TargetProtocol::Anthropic, "anthropic"),
        ] {
            for global in GLOBAL_CONTROL_KEYS {
                assert!(
                    is_reserved_key(global, target),
                    "{label}: global control key '{global}' must be reserved"
                );
            }
            assert!(
                !reserved_keys(target).is_empty(),
                "{label}: reserved set must not be empty"
            );
        }
        // Protocol-owned fields are reserved for their own target (drift guard
        // for the hand-written union constants) …
        assert!(is_reserved_key(
            "parallel_tool_calls",
            TargetProtocol::OpenAiChat
        ));
        assert!(is_reserved_key(
            "max_output_tokens",
            TargetProtocol::OpenAiResponses
        ));
        assert!(is_reserved_key("top_k", TargetProtocol::Anthropic));
        // … and only for their own target.
        assert!(!is_reserved_key("top_k", TargetProtocol::OpenAiChat));
        assert!(!is_reserved_key(
            "parallel_tool_calls",
            TargetProtocol::Anthropic
        ));
    }

    #[test]
    fn sanitize_is_idempotent_and_case_insensitive() {
        let once = sanitize_extra(
            map(&[
                ("System", json!("x")),
                ("TOP_K", json!(40)),
                ("x_keep", json!(1)),
            ]),
            TargetProtocol::Anthropic,
        );
        assert_eq!(once.len(), 1);
        assert!(once.contains_key("x_keep"));
        let twice = sanitize_extra(once.clone(), TargetProtocol::Anthropic);
        assert_eq!(twice, once);
    }

    #[test]
    fn oversized_value_is_dropped_and_entry_cap_is_applied() {
        let oversized = "A".repeat(MAX_EXTRA_VALUE_BYTES);
        let out = sanitize_extra(
            map(&[("x_big", json!(oversized)), ("x_small", json!("ok"))]),
            TargetProtocol::OpenAiChat,
        );
        assert!(!out.contains_key("x_big"));
        assert!(out.contains_key("x_small"));

        let many: HashMap<String, Value> = (0..MAX_EXTRA_ENTRIES + 10)
            .map(|i| (format!("x_ext_{i:03}"), json!(i)))
            .collect();
        let capped = sanitize_extra(many, TargetProtocol::OpenAiChat);
        assert_eq!(capped.len(), MAX_EXTRA_ENTRIES);
    }
}
