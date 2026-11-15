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
use std::collections::HashMap;
use uuid::Uuid;

use crate::common::{sanitize_wire_tool_name, ReasoningEffort};
use crate::error::Result;
use crate::openai::chat::{
    ChatChunkChoice, ChatChunkDelta, ChatCompletionChunk, ChatCompletionRequest, ChatMessage,
    CompletionTokensDetails, ContentPart, FinishReason, FunctionCallChunk, MessageContent,
    PromptTokensDetails, ResponseFormat, ToolCallChunk, ToolChoice, Usage,
};
use crate::translator::antigravity::{
    convert_tools_to_gemini, parse_inline_data_part, push_or_merge_turn, sanitize_gemini_schema,
};

/// Sentinel accepted by the native API in place of a real `thoughtSignature` on
/// replayed `functionCall` parts (probe-verified: without it, Gemini 3 rejects
/// multi-turn tool replay with `400 ... missing a thought_signature in
/// functionCall parts`). Same value the Antigravity translator sends.
const THOUGHT_SIGNATURE_SENTINEL: &str = "skip_thought_signature_validator";

// ---------------------------------------------------------------------------
// Thinking config
// ---------------------------------------------------------------------------

fn is_gemini_25(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.contains("gemini-2.5") || lower.contains("gemini-2-5")
}

/// Explicit-effort → `generationConfig.thinkingConfig` for the native Gemini
/// API. Returns `None` when the caller did not ask for a thinking level, so the
/// legacy wire shape is preserved.
///
/// Wire shape by family (both probe-accepted):
/// - `gemini-2.5*`: `thinkingBudget` (1024 / 4096 / 16384 / 24576 tokens)
/// - everything newer (`gemini-3.x`, `gemini-4.x`): `thinkingLevel`
///   (`low` / `medium` / `high`)
///
/// `includeThoughts` always accompanies the numeric/level field so thought
/// parts are actually returned (and land in `reasoning_content`).
pub fn gemini_thinking_config(model: &str, thinking: Option<ReasoningEffort>) -> Option<Value> {
    let effort = thinking?;

    if is_gemini_25(model) {
        return Some(match effort {
            ReasoningEffort::Off => json!({ "includeThoughts": false, "thinkingBudget": 0 }),
            ReasoningEffort::Low => json!({ "includeThoughts": true, "thinkingBudget": 1024 }),
            ReasoningEffort::Medium => json!({ "includeThoughts": true, "thinkingBudget": 4096 }),
            ReasoningEffort::High => json!({ "includeThoughts": true, "thinkingBudget": 16384 }),
            ReasoningEffort::Max => json!({ "includeThoughts": true, "thinkingBudget": 24576 }),
        });
    }

    // Gemini 3+: depth is a named level. `Off` still needs a level because the
    // field is what the 3.x backend understands; `includeThoughts: false` is
    // what actually suppresses thought output.
    let level = match effort {
        ReasoningEffort::Off => {
            return Some(json!({ "includeThoughts": false, "thinkingLevel": "low" }))
        }
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High | ReasoningEffort::Max => "high",
    };
    Some(json!({ "includeThoughts": true, "thinkingLevel": level }))
}

// ---------------------------------------------------------------------------
// Request: Chat → Gemini generateContent
// ---------------------------------------------------------------------------

fn audio_format_mime(format: &str) -> &'static str {
    match format.trim().to_ascii_lowercase().as_str() {
        "wav" => "audio/wav",
        "mp3" => "audio/mp3",
        "aac" => "audio/aac",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/m4a",
        _ => "audio/wav",
    }
}

/// Map one OpenAI chat content payload to Gemini `parts`. Multimodal payloads
/// reuse the proven `parse_inline_data_part` normalizer (data URI → `inlineData`,
/// text-document MIME → decoded `text` part).
fn content_to_gemini_parts(content: &MessageContent) -> Vec<Value> {
    match content {
        MessageContent::Text(text) => {
            if text.trim().is_empty() {
                Vec::new()
            } else {
                vec![json!({ "text": text })]
            }
        }
        MessageContent::Parts(parts) => {
            let mut out = Vec::new();
            for part in parts {
                match part {
                    ContentPart::Text { text } => {
                        if !text.trim().is_empty() {
                            out.push(json!({ "text": text }));
                        }
                    }
                    ContentPart::ImageUrl { image_url } => {
                        if let Some(inline) = parse_inline_data_part(&image_url.url, "image/jpeg") {
                            out.push(inline);
                        }
                    }
                    ContentPart::InputAudio { input_audio } => {
                        let mime = audio_format_mime(&input_audio.format);
                        if let Some(inline) = parse_inline_data_part(&input_audio.data, mime) {
                            out.push(inline);
                        }
                    }
                    ContentPart::VideoUrl { video_url } => {
                        if let Some(inline) = parse_inline_data_part(&video_url.url, "video/mp4") {
                            out.push(inline);
                        }
                    }
                    ContentPart::File { file } => {
                        if let Some(ref url) = file.file_url {
                            if let Some(inline) = parse_inline_data_part(url, "application/pdf") {
                                out.push(inline);
                            }
                        }
                    }
                }
            }
            out
        }
    }
}

/// Split a tool-result payload into `(joined_text, inline_parts)`: Gemini's
/// `functionResponse` only carries an object, so inline media rides alongside
/// it as sibling parts of the same user turn.
fn split_text_and_inline(content: &MessageContent) -> (String, Vec<Value>) {
    let mut texts = Vec::new();
    let mut inline = Vec::new();
    for part in content_to_gemini_parts(content) {
        match part.get("text").and_then(|v| v.as_str()) {
            Some(text) => texts.push(text.to_string()),
            None => inline.push(part),
        }
    }
    (texts.join("\n"), inline)
}

/// `functionResponse.response` must be a JSON object on the wire; keep objects
/// verbatim and wrap scalars/arrays under `response`.
fn function_response_object(text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(other) => json!({ "response": other }),
        Err(_) => json!({ "response": text }),
    }
}

/// `functionCall.args` must be a Struct; keep objects verbatim and wrap
/// non-object payloads under `value` so a malformed replay can't 400.
fn function_call_args(raw: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(other) => json!({ "value": other }),
        Err(_) => json!({}),
    }
}

fn tool_choice_to_tool_config(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Mode(mode) => {
            let mode = match mode.trim().to_ascii_lowercase().as_str() {
                "none" => "NONE",
                "required" | "any" => "ANY",
                "auto" => "AUTO",
                // Unknown modes fall back to the upstream default instead of
                // failing the request (Gemini would 400 on a bogus enum).
                _ => "AUTO",
            };
            json!({ "functionCallingConfig": { "mode": mode } })
        }
        ToolChoice::Specific { function, .. } => json!({
            "functionCallingConfig": {
                "mode": "ANY",
                "allowedFunctionNames": [sanitize_wire_tool_name(&function.name)],
            }
        }),
    }
}

fn gemini_generation_config(
    req: &ChatCompletionRequest,
    model: &str,
    thinking: Option<ReasoningEffort>,
) -> Option<Value> {
    let mut config = serde_json::Map::new();

    if let Some(temperature) = req.temperature {
        config.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = req.top_p {
        config.insert("topP".to_string(), json!(top_p));
    }
    // `topK` has no first-class Chat field; honor the passthrough spellings only
    // (everything else in `extra` is intentionally dropped — Gemini rejects
    // unknown body fields with 400).
    if let Some(top_k) = req
        .extra
        .get("top_k")
        .or_else(|| req.extra.get("topK"))
        .and_then(|v| v.as_u64())
    {
        config.insert("topK".to_string(), json!(top_k));
    }
    if let Some(max_tokens) = req.max_tokens.or(req.max_completion_tokens) {
        config.insert("maxOutputTokens".to_string(), json!(max_tokens));
    }
    if let Some(stop) = req.stop.as_ref() {
        let sequences = stop.as_slice();
        if !sequences.is_empty() {
            config.insert("stopSequences".to_string(), json!(sequences));
        }
    }
    if let Some(seed) = req.seed {
        config.insert("seed".to_string(), json!(seed));
    }
    if let Some(format) = req.response_format.as_ref() {
        match format {
            ResponseFormat::Text => {}
            ResponseFormat::JsonObject => {
                config.insert("responseMimeType".to_string(), json!("application/json"));
            }
            ResponseFormat::JsonSchema { json_schema } => {
                config.insert("responseMimeType".to_string(), json!("application/json"));
                if let Some(schema) = json_schema.schema.as_ref() {
                    config.insert("responseSchema".to_string(), sanitize_gemini_schema(schema));
                }
            }
        }
    }
    if let Some(thinking_cfg) = gemini_thinking_config(model, thinking) {
        config.insert("thinkingConfig".to_string(), thinking_cfg);
    }

    if config.is_empty() {
        None
    } else {
        Some(Value::Object(config))
    }
}

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
    req: &ChatCompletionRequest,
    model: &str,
    thinking: Option<ReasoningEffort>,
) -> Result<Value> {
    let mut contents: Vec<Value> = Vec::new();
    let mut system_instruction_parts: Vec<Value> = Vec::new();
    let mut tool_id_to_name: HashMap<String, String> = HashMap::new();

    // Gemini's `functionResponse.name` is mandatory: resolve tool results back
    // to the declared tool name through the assistant's tool_call ids first.
    for msg in &req.messages {
        if let ChatMessage::Assistant(m) = msg {
            if let Some(ref tool_calls) = m.tool_calls {
                for tc in tool_calls {
                    tool_id_to_name
                        .insert(tc.id.clone(), sanitize_wire_tool_name(&tc.function.name));
                }
            }
        }
    }

    for msg in &req.messages {
        match msg {
            ChatMessage::System(m) => {
                let text = m.content.as_plain_text();
                if !text.trim().is_empty() {
                    system_instruction_parts.push(json!({ "text": text }));
                }
            }
            ChatMessage::Developer(m) => {
                let text = m.content.as_plain_text();
                if !text.trim().is_empty() {
                    system_instruction_parts.push(json!({ "text": text }));
                }
            }
            ChatMessage::User(m) => {
                let parts = content_to_gemini_parts(&m.content);
                push_or_merge_turn(&mut contents, "user", parts);
            }
            ChatMessage::Assistant(m) => {
                let mut parts = Vec::new();
                if let Some(ref content) = m.content {
                    let text = content.as_plain_text();
                    if !text.trim().is_empty() {
                        parts.push(json!({ "text": text }));
                    }
                }
                if let Some(ref tool_calls) = m.tool_calls {
                    for tc in tool_calls {
                        let name = sanitize_wire_tool_name(&tc.function.name);
                        parts.push(json!({
                            "functionCall": {
                                "name": name,
                                "args": function_call_args(&tc.function.arguments),
                            },
                            // Mandatory for native Gemini 3 multi-turn replay.
                            "thoughtSignature": THOUGHT_SIGNATURE_SENTINEL,
                        }));
                    }
                }
                push_or_merge_turn(&mut contents, "model", parts);
            }
            ChatMessage::Tool(m) => {
                let func_name = m
                    .name
                    .as_deref()
                    .filter(|n| !n.trim().is_empty())
                    .map(|n| sanitize_wire_tool_name(n))
                    .or_else(|| {
                        tool_id_to_name
                            .get(&m.tool_call_id)
                            .cloned()
                            .filter(|n| !n.trim().is_empty())
                    })
                    .unwrap_or_else(|| {
                        // Protobuf rejects an empty name; synthesize a stable one.
                        let id = m.tool_call_id.trim();
                        if id.is_empty() {
                            "tool_result".to_string()
                        } else {
                            format!("tool_{id}")
                        }
                    });
                let (text, inline_parts) = split_text_and_inline(&m.content);
                let mut turn_parts = vec![json!({
                    "functionResponse": {
                        "name": func_name,
                        "response": function_response_object(&text),
                    }
                })];
                turn_parts.extend(inline_parts);
                push_or_merge_turn(&mut contents, "user", turn_parts);
            }
            ChatMessage::Function(m) => {
                let func_name = if m.name.trim().is_empty() {
                    "tool_result".to_string()
                } else {
                    sanitize_wire_tool_name(&m.name)
                };
                let text = m.content.clone().unwrap_or_default();
                push_or_merge_turn(
                    &mut contents,
                    "user",
                    vec![json!({
                        "functionResponse": {
                            "name": func_name,
                            "response": function_response_object(&text),
                        }
                    })],
                );
            }
        }
    }

    let mut body = json!({ "contents": contents });

    if !system_instruction_parts.is_empty() {
        body["systemInstruction"] = json!({ "parts": system_instruction_parts });
    }

    if let Some(ref tools_def) = req.tools {
        if let Some(gemini_tools) = convert_tools_to_gemini(tools_def) {
            body["tools"] = gemini_tools;
            // Absent `tool_choice` means AUTO for Gemini too; send it explicitly
            // so the intended mode is visible in the wire body.
            body["toolConfig"] = match req.tool_choice.as_ref() {
                Some(choice) => tool_choice_to_tool_config(choice),
                None => json!({ "functionCallingConfig": { "mode": "AUTO" } }),
            };
        }
    }

    if let Some(generation_config) = gemini_generation_config(req, model, thinking) {
        body["generationConfig"] = generation_config;
    }

    // `n > 1` is unsupported by this route: one candidate is always returned,
    // so `candidateCount` is never emitted (a client asking for n=2 gets the
    // single candidate rather than an upstream 400).
    Ok(body)
}

// ---------------------------------------------------------------------------
// Response: Gemini → Chat
// ---------------------------------------------------------------------------

fn value_as_u32(v: &Value) -> Option<u32> {
    v.as_u64().map(|n| n.min(u32::MAX as u64) as u32)
}

/// Map Gemini `usageMetadata` to OpenAI `Usage`.
///
/// `thoughtsTokenCount` (reasoning) is billed as output by Gemini, so it is
/// folded into `completion_tokens` (probe-verified total =
/// prompt + candidates + thoughts) and also surfaced as
/// `completion_tokens_details.reasoning_tokens`.
fn usage_from_gemini_metadata(metadata: &Value) -> Usage {
    let prompt_tokens = metadata
        .get("promptTokenCount")
        .and_then(value_as_u32)
        .unwrap_or(0);
    let candidates_tokens = metadata
        .get("candidatesTokenCount")
        .and_then(value_as_u32)
        .unwrap_or(0);
    let thoughts_tokens = metadata
        .get("thoughtsTokenCount")
        .and_then(value_as_u32)
        .unwrap_or(0);
    let total_tokens = metadata
        .get("totalTokenCount")
        .and_then(value_as_u32)
        .unwrap_or(
            prompt_tokens
                .saturating_add(candidates_tokens)
                .saturating_add(thoughts_tokens),
        );
    let cached_tokens = metadata
        .get("cachedContentTokenCount")
        .and_then(value_as_u32)
        .unwrap_or(0);

    Usage {
        prompt_tokens,
        completion_tokens: candidates_tokens.saturating_add(thoughts_tokens),
        total_tokens,
        prompt_tokens_details: Some(PromptTokensDetails {
            cached_tokens: Some(cached_tokens),
            audio_tokens: None,
        }),
        completion_tokens_details: Some(CompletionTokensDetails {
            reasoning_tokens: Some(thoughts_tokens),
            ..Default::default()
        }),
    }
}

fn finish_reason_from_gemini(reason: Option<&str>, has_tool_calls: bool) -> FinishReason {
    // Plain STOP (and no reason at all) is promoted to `tool_calls` when the
    // turn actually carried function calls — Gemini reports STOP for those too.
    let promoted = || {
        if has_tool_calls {
            FinishReason::ToolCalls
        } else {
            FinishReason::Stop
        }
    };
    match reason {
        Some("STOP") | None => promoted(),
        Some("MAX_TOKENS") => FinishReason::Length,
        Some("SAFETY")
        | Some("RECITATION")
        | Some("BLOCKLIST")
        | Some("PROHIBITED_CONTENT")
        | Some("SPII")
        | Some("IMAGE_SAFETY") => FinishReason::ContentFilter,
        Some("MALFORMED_FUNCTION_CALL") => FinishReason::Stop,
        Some(_) => promoted(),
    }
}

/// Map Gemini's `finishReason` to OpenAI's `FinishReason`. `has_tool_calls`
/// promotes a plain STOP to `tool_calls` (Gemini reports STOP after function
/// calls too).
pub fn map_gemini_finish_reason(reason: Option<&str>, has_tool_calls: bool) -> FinishReason {
    finish_reason_from_gemini(reason, has_tool_calls)
}

fn function_call_part_to_tool_call(fc: &Value) -> Value {
    let name = fc
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or_default()
        .to_string();
    let arguments = match fc.get("args") {
        Some(Value::String(s)) => s.clone(),
        Some(v) => serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()),
        None => "{}".to_string(),
    };
    // Gemini sometimes returns `functionCall.id` (parallel calls); keep it so
    // the client can round-trip the pairing. Only synthesize when absent.
    let id = fc
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));

    json!({
        "id": id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments,
        }
    })
}

/// Candidate `parts`, tolerating both the documented `content.parts` nesting
/// and a bare `candidate.parts` (observed shorthand in some frames/proxies).
fn candidate_parts(candidate: &Value) -> Option<&Vec<Value>> {
    candidate
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
        .or_else(|| candidate.get("parts").and_then(|p| p.as_array()))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Convert a native Gemini `generateContent` JSON response into an OpenAI Chat
/// completion value (route layer deserializes into `ChatCompletionResponse`).
///
/// Native shape: `{candidates: [{content: {parts: [...]}, finishReason}],
/// usageMetadata: {...}, modelVersion, responseId}`. There is no `response`
/// envelope (unlike Antigravity), but tolerate one defensively.
pub fn gemini_to_chat_response(resp: &Value, model: &str) -> Value {
    let target = resp.get("response").unwrap_or(resp);
    let response_id = target.get("responseId").and_then(|v| v.as_str());

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();
    let mut raw_finish: Option<&str> = None;

    let candidates = target.get("candidates").and_then(|v| v.as_array());
    if let Some(first) = candidates.and_then(|c| c.first()) {
        raw_finish = first.get("finishReason").and_then(|v| v.as_str());
        if let Some(parts) = candidate_parts(first) {
            for part in parts {
                let is_thought = part
                    .get("thought")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Some(chunk) = part.get("text").and_then(|t| t.as_str()) {
                    if is_thought {
                        reasoning.push_str(chunk);
                    } else {
                        text.push_str(chunk);
                    }
                }
                if let Some(fc) = part.get("functionCall") {
                    tool_calls.push(function_call_part_to_tool_call(fc));
                }
            }
        }
    }

    let has_tool_calls = !tool_calls.is_empty();
    let mut finish_reason = finish_reason_from_gemini(raw_finish, has_tool_calls);

    // A prompt-level block yields no candidates at all, only `promptFeedback`;
    // surface it as a content filter rather than a silent empty STOP.
    let no_candidates = candidates.map(|c| c.is_empty()).unwrap_or(true);
    if no_candidates
        && target
            .get("promptFeedback")
            .and_then(|f| f.get("blockReason"))
            .is_some()
    {
        finish_reason = FinishReason::ContentFilter;
    }

    let mut message = json!({
        "role": "assistant",
        "content": if text.is_empty() && has_tool_calls { Value::Null } else { json!(text) },
    });
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    if has_tool_calls {
        message["tool_calls"] = json!(tool_calls);
    }

    let mut out = json!({
        "id": gemini_chat_id(response_id),
        "object": "chat.completion",
        "created": now_secs(),
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": message,
                "finish_reason": finish_reason,
            }
        ],
    });

    if let Some(metadata) = target.get("usageMetadata").filter(|v| v.is_object()) {
        out["usage"] = json!(usage_from_gemini_metadata(metadata));
    }

    out
}

fn chunk_usage(container: &Value) -> Option<Usage> {
    container
        .get("usageMetadata")
        .filter(|v| v.is_object())
        .map(usage_from_gemini_metadata)
}

/// Convert one native Gemini SSE frame (`data: {candidates: ...}`) into an
/// OpenAI chat chunk. Returns `None` when the frame has no candidates (e.g.
/// usage-only or promptFeedback-only frames).
///
/// Tool-call `index` values are assigned per frame (0, 1, 2 …): Gemini emits a
/// turn's function calls together, so the index lines up with the final tool
/// call order. The signature intentionally carries no cross-frame state; a
/// client that receives function calls spread over several frames still gets
/// each call with its own id and complete arguments.
pub fn gemini_chunk_to_chat_chunk(
    chunk: &Value,
    model: &str,
    response_id: &str,
) -> Option<ChatCompletionChunk> {
    let target = chunk.get("response").unwrap_or(chunk);
    let candidates = target.get("candidates")?.as_array()?;
    let first = candidates.first()?;

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<ToolCallChunk> = Vec::new();

    if let Some(parts) = candidate_parts(first) {
        for part in parts {
            let is_thought = part
                .get("thought")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if let Some(chunk_text) = part.get("text").and_then(|t| t.as_str()) {
                if is_thought {
                    reasoning.push_str(chunk_text);
                } else {
                    text.push_str(chunk_text);
                }
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc.get("name").and_then(|n| n.as_str()).map(str::to_string);
                let arguments = match fc.get("args") {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(v) => Some(serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string())),
                    None => None,
                };
                let id = fc
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));
                tool_calls.push(ToolCallChunk {
                    index: tool_calls.len() as u32,
                    id: Some(id),
                    r#type: Some("function".to_string()),
                    function: Some(FunctionCallChunk { name, arguments }),
                });
            }
        }
    }

    let has_tool_calls = !tool_calls.is_empty();
    let finish_reason = first
        .get("finishReason")
        .and_then(|v| v.as_str())
        .map(|reason| finish_reason_from_gemini(Some(reason), has_tool_calls));

    let delta = ChatChunkDelta {
        role: None,
        content: if text.is_empty() { None } else { Some(text) },
        reasoning_content: if reasoning.is_empty() {
            None
        } else {
            Some(reasoning)
        },
        refusal: None,
        tool_calls: if has_tool_calls {
            Some(tool_calls)
        } else {
            None
        },
    };

    Some(ChatCompletionChunk {
        id: response_id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created: now_secs(),
        model: model.to_string(),
        choices: vec![ChatChunkChoice {
            index: 0,
            delta,
            finish_reason,
            logprobs: None,
        }],
        usage: chunk_usage(target),
        system_fingerprint: None,
        service_tier: None,
    })
}

/// Accept either a native `usageMetadata` object (optionally wrapped as
/// `{usageMetadata: {...}}`) or an already OpenAI-shaped `Usage` object, so the
/// executor can forward whatever frame it collected without re-shaping.
fn terminal_usage(raw: &Value) -> Option<Usage> {
    let metadata = raw.get("usageMetadata").unwrap_or(raw);
    let looks_gemini = metadata.get("promptTokenCount").is_some()
        || metadata.get("candidatesTokenCount").is_some()
        || metadata.get("thoughtsTokenCount").is_some()
        || metadata.get("totalTokenCount").is_some();
    if looks_gemini {
        return Some(usage_from_gemini_metadata(metadata));
    }
    if metadata.get("prompt_tokens").is_some() {
        return serde_json::from_value::<Usage>(metadata.clone()).ok();
    }
    None
}

/// Build the terminal chat chunk emitted when the upstream stream ends without
/// an explicit finish reason (Gemini normally sends one, but a truncated
/// connection must not leave downstream clients without a terminator).
pub fn gemini_terminal_chunk(
    model: &str,
    response_id: &str,
    usage: Option<Value>,
    finish_reason: FinishReason,
) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: response_id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created: now_secs(),
        model: model.to_string(),
        choices: vec![ChatChunkChoice {
            index: 0,
            delta: ChatChunkDelta::default(),
            finish_reason: Some(finish_reason),
            logprobs: None,
        }],
        usage: usage.as_ref().and_then(terminal_usage),
        system_fingerprint: None,
        service_tier: None,
    }
}

/// Stable, client-safe id for a Gemini response/stream. Gemini's `responseId`
/// is already unguessable; prefix it so it is recognizable as a chat id.
pub fn gemini_chat_id(response_id: Option<&str>) -> String {
    match response_id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => format!("chatcmpl-{id}"),
        None => format!("chatcmpl-{}", Uuid::new_v4().simple()),
    }
}
