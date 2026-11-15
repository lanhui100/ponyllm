//! Native Google Gemini API (`generativelanguage.googleapis.com/v1beta`)
//! request/response translation.
//!
//! This module is intentionally independent of the Antigravity envelope: the
//! Antigravity protocol wraps a Gemini-shaped inner request in
//! `{project, requestId, request: {...}}` and authenticates with OAuth, while
//! the Gemini Developer API accepts the inner body verbatim and authenticates
//! with `x-goog-api-key`. The Chat Completions request is the single canonical
//! pivot: Messages/Responses requests are converted to Chat first (existing
//! `anthropic_to_chat_request` / `responses_to_chat_request`), then to Gemini
//! here; Gemini responses are converted back to Chat and the route layer
//! projects Chat → the downstream protocol.

use serde_json::{json, Value};
use uuid::Uuid;

use crate::common::ReasoningEffort;
use crate::error::Result;
use crate::openai::chat::{
    ChatCompletionChunk, ChatCompletionRequest, FinishReason, FunctionCallChunk, ToolCallChunk,
};

/// Build a native Gemini `generateContent` request body from an OpenAI Chat
/// request. The returned value is the HTTP body only (`contents`,
/// `systemInstruction`, `tools`, `toolConfig`, `generationConfig`); the model
/// travels in the URL path and must NOT appear in the body.
///
/// `thinking` carries the caller's *explicit* effort (route layer has already
/// applied model ceiling): `None` omits `thinkingConfig` entirely (upstream
/// default), `Some(Off)` explicitly disables thought output, active tiers map to
/// `thinkingLevel` (Gemini 3) or `thinkingBudget` (2.5).
pub fn chat_to_gemini_request(
    _req: &ChatCompletionRequest,
    _model: &str,
    _thinking: Option<ReasoningEffort>,
) -> Result<Value> {
    unimplemented!("gemini request translation")
}

/// Convert a native Gemini `generateContent` JSON response into an OpenAI Chat
/// completion value (route layer deserializes into `ChatCompletionResponse`).
///
/// Native shape: `{candidates: [{content: {parts: [...]}, finishReason}],
/// usageMetadata: {...}, modelVersion, responseId}`. There is no `response`
/// envelope (unlike Antigravity), but tolerate one defensively.
pub fn gemini_to_chat_response(_resp: &Value, _model: &str) -> Value {
    unimplemented!("gemini response translation")
}

/// Convert one native Gemini SSE frame (`data: {candidates: ...}`) into an
/// OpenAI chat chunk. Returns `None` when the frame has no candidates (e.g.
/// usage-only or promptFeedback-only frames).
pub fn gemini_chunk_to_chat_chunk(
    _chunk: &Value,
    _model: &str,
    _response_id: &str,
) -> Option<ChatCompletionChunk> {
    unimplemented!("gemini stream chunk translation")
}

/// Map Gemini's `finishReason` to OpenAI's `FinishReason`. `has_tool_calls`
/// promotes a plain STOP to `tool_calls` (Gemini reports STOP after function
/// calls too).
pub fn map_gemini_finish_reason(
    _reason: Option<&str>,
    _has_tool_calls: bool,
) -> FinishReason {
    unimplemented!("gemini finish-reason mapping")
}

/// Build the terminal chat chunk emitted when the upstream stream ends without
/// an explicit finish reason (Gemini normally sends one, but a truncated
/// connection must not leave downstream clients without a terminator).
pub fn gemini_terminal_chunk(
    _model: &str,
    _response_id: &str,
    _usage: Option<Value>,
    _finish_reason: FinishReason,
) -> ChatCompletionChunk {
    unimplemented!("gemini terminal chunk")
}

/// Explicit-effort → `generationConfig.thinkingConfig` for the native Gemini
/// API. Returns `None` when the caller did not ask for a thinking level, so the
/// legacy wire shape is preserved.
pub fn gemini_thinking_config(
    _model: &str,
    _thinking: Option<ReasoningEffort>,
) -> Option<Value> {
    unimplemented!("gemini thinking config")
}

/// Stable, client-safe id for a Gemini response/stream. Gemini's `responseId`
/// is already unguessable; prefix it so it is recognizable as a chat id.
pub fn gemini_chat_id(response_id: Option<&str>) -> String {
    match response_id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => format!("chatcmpl-{id}"),
        None => format!("chatcmpl-{}", Uuid::new_v4().simple()),
    }
}

// Keep imports referenced until the implementation lands (the executor and
// route wiring are developed in parallel against this signature set).
#[allow(dead_code)]
fn _type_anchors(
    _c: ChatCompletionChunk,
    _f: FunctionCallChunk,
    _t: ToolCallChunk,
    _j: Value,
) -> Value {
    json!(null)
}
