//! Cross-protocol `extra` sanitization boundary (P0 safety fix).
//!
//! CONTRACT SKELETON ONLY: no business logic is implemented in this module.
//! The symbols below are frozen so downstream crates can compile against the
//! final API while the filtering behaviour lands in a follow-up change.
//!
//! Rationale: request bodies deserialized with `#[serde(flatten)] extra` accept
//! arbitrary caller-controlled JSON keys. Forwarding that map verbatim into a
//! *different* protocol lets a caller inject fields owned by the target wire
//! format. This module is the single choke point that decides which names and
//! which sizes are allowed to cross that boundary.

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

/// Field names owned by the Chat wire format; `extra` must never re-inject them.
pub const RESERVED_CHAT: &[&str] = &[];

/// Field names owned by the Responses wire format; `extra` must never re-inject them.
pub const RESERVED_RESPONSES: &[&str] = &[];

/// Field names owned by the Anthropic wire format; `extra` must never re-inject them.
pub const RESERVED_ANTHROPIC: &[&str] = &[];

/// Hard cap on caller-supplied `extra` entries that may reach upstream.
pub const MAX_EXTRA_ENTRIES: usize = 64;

/// Hard cap on a single `extra` value's serialized size, in bytes.
pub const MAX_EXTRA_VALUE_BYTES: usize = 16 * 1024;

/// Reserved field names owned by `target`'s wire format.
///
/// Frozen contract (not yet implemented): returns [`RESERVED_CHAT`],
/// [`RESERVED_RESPONSES`], or [`RESERVED_ANTHROPIC`] for the matching target.
#[allow(unused_variables)]
pub fn reserved_keys(target: TargetProtocol) -> &'static [&'static str] {
    unimplemented!()
}

/// Filters a flattened `extra` map down to the names `target` tolerates.
///
/// Frozen contract (not yet implemented):
/// - names reserved by the target protocol are dropped;
/// - maps holding more than [`MAX_EXTRA_ENTRIES`] entries, or values whose
///   serialized form exceeds [`MAX_EXTRA_VALUE_BYTES`], are rejected;
/// - the returned map is safe to forward to the target wire format unchanged.
#[allow(unused_variables)]
pub fn sanitize_extra(
    extra: HashMap<String, Value>,
    target: TargetProtocol,
) -> HashMap<String, Value> {
    unimplemented!()
}
