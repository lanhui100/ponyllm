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
//! Contract (frozen, v5):
//! - `reserved_keys(target)` = the target's own explicit request fields
//!   ∪ [`GLOBAL_CONTROL_KEYS`], compared case-insensitively; any key containing
//!   a non-ASCII byte is dropped outright (Unicode simple-fold aliases such as
//!   `ſystem` / `ma\u{212A}x_tokens` must never alias a reserved field);
//! - [`sanitize_extra`] additionally caps a key at [`MAX_EXTRA_KEY_BYTES`] and
//!   an entry at `key.len() + serialized value len ≤ MAX_EXTRA_VALUE_BYTES`,
//!   measured with an early-aborting bounded writer;
//! - everything else passes through byte-for-byte.

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
    "safety_identifier",
    "stream_options",
];

/// Hard cap on caller-supplied `extra` entries that may reach upstream.
pub const MAX_EXTRA_ENTRIES: usize = 64;

/// Hard cap on a single `extra` value's serialized size, in bytes.
pub const MAX_EXTRA_VALUE_BYTES: usize = 16 * 1024;

/// Hard cap on a single `extra` key's length, in bytes.
///
/// Key bytes are attacker-controlled too: without this cap a multi-megabyte key
/// carrying a 1-byte value would slip past a value-only budget.
pub const MAX_EXTRA_KEY_BYTES: usize = 256;

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

/// Whether `key` must be dropped for `target`.
///
/// Two rules, both fail-closed:
/// 1. any key containing a non-ASCII byte is reserved — Unicode simple-fold
///    aliases (`ſystem` U+017F, `ma\u{212A}x_tokens` U+212A KELVIN SIGN) fold
///    onto real field names in upstream case-folding implementations, so no
///    alias may cross the boundary at all (no folding, no aliasing);
/// 2. otherwise the name is compared case-insensitively (ASCII only, which is
///    now total because rule 1 removed every non-ASCII key).
pub fn is_reserved_key(key: &str, target: TargetProtocol) -> bool {
    if !key.is_ascii() {
        return true;
    }
    reserved_keys(target)
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(key))
}

/// Counts written bytes and fails as soon as the budget is exceeded, so an
/// oversized value is rejected *while* serializing — the full serialization is
/// never materialized (a 32 MiB value must not cost a 32 MiB copy).
struct BoundedCounter {
    written: usize,
    limit: usize,
}

impl std::io::Write for BoundedCounter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.written.checked_add(buf.len()) {
            Some(total) if total <= self.limit => {
                self.written = total;
                Ok(buf.len())
            }
            _ => Err(std::io::Error::other("extra value exceeds byte budget")),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serialized byte length of a single `extra` value, measuring at most `budget`
/// bytes; `None` when the value cannot be serialized or exceeds the budget.
fn serialized_len_within(value: &Value, budget: usize) -> Option<usize> {
    let mut counter = BoundedCounter {
        written: 0,
        limit: budget,
    };
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.written)
}

/// Filters a flattened `extra` map down to the names `target` tolerates.
///
/// Rules (all fail-closed, none panicking):
/// - names reserved by `target` are dropped, and so is any non-ASCII key
///   (`is_reserved_key`);
/// - a key longer than [`MAX_EXTRA_KEY_BYTES`] is dropped;
/// - an entry whose `key.len() + serialized value length` exceeds
///   [`MAX_EXTRA_VALUE_BYTES`] is dropped, measured with an early-aborting
///   bounded writer so no oversized serialization is ever materialized;
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
        .filter(|(key, _)| key.len() <= MAX_EXTRA_KEY_BYTES)
        .filter(|(key, value)| {
            let budget = MAX_EXTRA_VALUE_BYTES.saturating_sub(key.len());
            serialized_len_within(value, budget).is_some()
        })
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
    fn non_ascii_keys_and_overlong_keys_are_dropped() {
        let overlong = "k".repeat(MAX_EXTRA_KEY_BYTES + 1);
        let out = sanitize_extra(
            map(&[
                ("\u{017F}ystem", json!("fold-alias")),
                ("ma\u{212A}x_tokens", json!(9)),
                ("x_ünicode", json!("non-ascii")),
                (overlong.as_str(), json!(1)),
                ("x_keep", json!(2)),
            ]),
            TargetProtocol::OpenAiChat,
        );
        assert_eq!(
            out.len(),
            1,
            "only the ASCII, in-budget key survives: {out:?}"
        );
        assert_eq!(out.get("x_keep"), Some(&json!(2)));
    }

    #[test]
    fn key_bytes_count_against_the_size_budget() {
        let key = "a".repeat(200);
        let kept = sanitize_extra(
            map(&[(key.as_str(), json!("b".repeat(15000)))]),
            TargetProtocol::OpenAiChat,
        );
        assert_eq!(kept.len(), 1, "key(200)+value(15000) is inside 16 KiB");
        let dropped = sanitize_extra(
            map(&[(key.as_str(), json!("c".repeat(16350)))]),
            TargetProtocol::OpenAiChat,
        );
        assert!(dropped.is_empty(), "key bytes must count toward the budget");
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
