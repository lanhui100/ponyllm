use std::collections::HashMap;
use serde_json::{json, Value};
use uuid::Uuid;
use crate::common::ReasoningEffort;
use crate::error::Result;
use crate::openai::chat::{
    ChatCompletionChunk, ChatCompletionRequest, ChatChunkChoice, ChatChunkDelta,
    ChatMessage, FinishReason, Usage,
};
use crate::anthropic::messages::MessageRequest;

/// Generate Antigravity CLI compliant requestId: agent/{uuid}/{timestamp_ms}/{session_id}/{step}
pub fn generate_antigravity_request_id(session_id: &str, step: u32) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("agent/{}/{}/{}/{}", Uuid::new_v4(), now_ms, session_id, step)
}

/// Synthesize a session ID from the first user prompt text or fallback to random.
/// `salt` isolates identical prompts across keys/accounts (B7): without it,
/// the same first prompt under different credentials yields the same
/// sessionId, a cross-account clustering signal. An empty salt preserves the
/// legacy output exactly (embedded SDK path).
pub fn extract_or_generate_session_id(first_text: Option<&str>, salt: &str) -> String {
    if let Some(text) = first_text {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            // Simple deterministic integer hash prefixed with negative sign to match CLI convention
            let mut hash: u64 = 5381;
            for b in salt.bytes().chain(trimmed.bytes()) {
                hash = ((hash << 5).wrapping_add(hash)).wrapping_add(b as u64);
            }
            let val = (hash & 0x7FFFFFFFFFFFFFFF) as i64;
            return format!("-{}", val);
        }
    }
    format!("-{}", (Uuid::new_v4().as_u128() % 9_000_000_000_000_000_000) as i64)
}

/// Helper to parse and normalize data URI or raw base64 data for Gemini inlineData
pub fn parse_inline_data_part(raw_url_or_data: &str, default_mime: &str) -> Option<Value> {
    let raw = raw_url_or_data.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(stripped) = raw.strip_prefix("data:") {
        if let Some((mime, b64)) = stripped.split_once(";base64,") {
            let mime_clean = mime.trim();
            let b64_clean = b64.trim();
            // If the mime type is a text-based document (e.g. text/plain, text/csv, application/json),
            // Gemini inlineData does not accept it (causes 400 INVALID_ARGUMENT: Unsupported MIME type).
            // Try decoding it to utf-8 text.
            if is_text_document_mime(mime_clean) {
                if let Ok(bytes) = base64_decode(b64_clean) {
                    if let Ok(text) = String::from_utf8(bytes) {
                        return Some(json!({ "text": text }));
                    }
                }
                // If decoding fails, do NOT send as inlineData because Gemini rejects text MIME types
                return Some(json!({ "text": format!("[Document: {} (unparsed)]", mime_clean) }));
            }
            return Some(json!({
                "inlineData": {
                    "mimeType": mime_clean,
                    "data": b64_clean
                }
            }));
        }
    }

    // If it's already a raw base64 string or URI
    Some(json!({
        "inlineData": {
            "mimeType": default_mime,
            "data": raw
        }
    }))
}

/// Simple base64 decoding helper without external dependencies
fn base64_decode(input: &str) -> std::result::Result<Vec<u8>, ()> {
    // Quick base64 decode implementation using standard & url-safe base64 table
    const TABLE: [i8; 256] = {
        let mut t = [-1i8; 256];
        let mut i = 0usize;
        while i < 26 {
            t[(b'A' + i as u8) as usize] = i as i8;
            t[(b'a' + i as u8) as usize] = (i + 26) as i8;
            i += 1;
        }
        let mut d = 0usize;
        while d < 10 {
            t[(b'0' + d as u8) as usize] = (d + 52) as i8;
            d += 1;
        }
        t[b'+' as usize] = 62;
        t[b'-' as usize] = 62; // URL-safe alias
        t[b'/' as usize] = 63;
        t[b'_' as usize] = 63; // URL-safe alias
        t
    };

    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;

    for &b in bytes {
        if b == b'=' || b.is_ascii_whitespace() {
            continue;
        }
        let val = TABLE[b as usize];
        if val < 0 {
            return Err(());
        }
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

fn is_text_document_mime(mime: &str) -> bool {
    let m = mime.to_ascii_lowercase();
    m.starts_with("text/")
        || m == "application/json"
        || m == "application/xml"
        || m == "application/x-yaml"
        || m == "application/yaml"
        || m == "application/javascript"
}

fn map_audio_format_to_mime(format: &str) -> &'static str {
    match format.to_ascii_lowercase().as_str() {
        "wav" => "audio/wav",
        "mp3" => "audio/mp3",
        "aac" => "audio/aac",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/m4a",
        _ => "audio/wav",
    }
}

/// Map an explicit ponyllm [`ReasoningEffort`] to an Antigravity
/// `thinkingConfig` value, mirroring the reference `gcli2api` behavior:
///
/// - `None` (caller passed no explicit effort): return `None` so the legacy
///   wire shape is preserved byte-for-byte (backend default applies).
/// - `Off`: `{"includeThoughts": false}`, no budget (suppress thought return).
/// - Active (`Low`/`Medium`/`High`): `{"includeThoughts": true}` plus a
///   `thinkingBudget` of 1024 / 4096 / 16384 — except for `gemini-3*` models,
///   where the backend selects depth from the model route (`-high`/`-low`
///   suffix) and rejects/conflicts on an explicit budget, so only
///   `includeThoughts` is sent (reference strips `thinkingBudget` /
///   `thinkingLevel` there too).
pub fn antigravity_thinking_config(model: &str, thinking: Option<ReasoningEffort>) -> Option<Value> {
    let effort = thinking?;
    if effort == ReasoningEffort::Off {
        if !model.to_ascii_lowercase().contains("gemini-3") {
            return Some(json!({ "includeThoughts": false, "thinkingBudget": 0 }));
        } else {
            return Some(json!({ "includeThoughts": false }));
        }
    }
    let mut cfg = json!({ "includeThoughts": true });
    if !model.to_ascii_lowercase().contains("gemini-3") {
        let budget = match effort {
            ReasoningEffort::Low => 1024,
            ReasoningEffort::Medium => 4096,
            ReasoningEffort::High => 16384,
            ReasoningEffort::Off => unreachable!(),
        };
        cfg["thinkingBudget"] = json!(budget);
    }
    Some(cfg)
}

/// Ensure `generationConfig.maxOutputTokens` can accommodate an explicit or
/// implicit thinking budget: the backend couples the two and truncates the answer
/// (early `max_tokens` stop) when the budget exceeds the output cap.
/// Only raises an explicitly-set cap; absent caps keep backend defaults.
fn clamp_max_output_for_thinking_budget(
    gen_config: &mut Value,
    thinking_cfg: &Value,
    model: &str,
    thinking: Option<ReasoningEffort>,
) {
    let mut min_budget = thinking_cfg
        .get("thinkingBudget")
        .and_then(|v| v.as_u64());

    // Gemini 3 models omit thinkingBudget from wire format because the backend
    // selects depth from model route (-high/-low), but they still consume output tokens.
    // Ensure gen_config has sufficient headroom for implicit Gemini 3 thinking budgets.
    if min_budget.is_none() && model.to_ascii_lowercase().contains("gemini-3") {
        let is_high = model.to_ascii_lowercase().contains("-high")
            || thinking == Some(ReasoningEffort::High);
        let is_low = model.to_ascii_lowercase().contains("-low")
            || thinking == Some(ReasoningEffort::Low);
        if is_high {
            min_budget = Some(16384);
        } else if is_low {
            min_budget = Some(2048);
        } else {
            min_budget = Some(8192);
        }
    }

    let Some(budget) = min_budget else { return };
    if budget == 0 {
        return;
    }
    if let Some(cap) = gen_config
        .get_mut("maxOutputTokens")
        .and_then(|v| v.as_u64())
    {
        if cap <= budget {
            gen_config["maxOutputTokens"] = json!(budget + 1024);
        }
    }
}
fn push_or_merge_turn(contents: &mut Vec<Value>, role: &str, mut parts: Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = contents.last_mut() {
        if last.get("role").and_then(|r| r.as_str()) == Some(role) {
            if let Some(existing_parts) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                existing_parts.append(&mut parts);
                return;
            }
        }
    }
    contents.push(json!({
        "role": role,
        "parts": parts,
    }));
}

fn convert_tools_to_gemini(tools: &[crate::openai::chat::ToolDefinition]) -> Option<Value> {
    let mut decls = Vec::new();
    for t in tools {
        if t.r#type == "function" {
            let mut decl = json!({
                "name": t.function.name,
            });
            if let Some(ref desc) = t.function.description {
                decl["description"] = json!(desc);
            }
            if let Some(ref params) = t.function.parameters {
                decl["parameters"] = params.clone();
            }
            decls.push(decl);
        }
    }
    if decls.is_empty() {
        None
    } else {
        Some(json!([{
            "functionDeclarations": decls
        }]))
    }
}

fn permissive_safety_settings() -> Value {
    json!([
        {
            "category": "HARM_CATEGORY_HARASSMENT",
            "threshold": "BLOCK_NONE"
        },
        {
            "category": "HARM_CATEGORY_HATE_SPEECH",
            "threshold": "BLOCK_NONE"
        },
        {
            "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
            "threshold": "BLOCK_NONE"
        },
        {
            "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
            "threshold": "BLOCK_NONE"
        },
        {
            "category": "HARM_CATEGORY_CIVIC_INTEGRITY",
            "threshold": "BLOCK_NONE"
        }
    ])
}

fn convert_anthropic_tools_to_gemini(tools: &[crate::anthropic::messages::AnthropicTool]) -> Option<Value> {
    let mut decls = Vec::new();
    for t in tools {
        let decl = json!({
            "name": t.name,
            "description": t.description,
            "parameters": t.input_schema,
        });
        decls.push(decl);
    }
    if decls.is_empty() {
        None
    } else {
        Some(json!([{
            "functionDeclarations": decls
        }]))
    }
}

/// Convert OpenAI ChatCompletionRequest into Antigravity CLI envelope.
/// `thinking` carries the caller's *explicit* effort request (`None` keeps the
/// legacy wire shape); ceiling enforcement happens at the route layer via
/// `ModelThinkingSpec::resolve`.
pub fn chat_to_antigravity_request(
    req: &ChatCompletionRequest,
    model: &str,
    project_id: &str,
    thinking: Option<ReasoningEffort>,
    session_salt: &str,
) -> Result<Value> {
    let mut contents = Vec::new();
    let mut first_user_text: Option<String> = None;
    let mut system_instruction_parts = Vec::new();
    let mut tool_id_to_name: HashMap<String, String> = HashMap::new();

    for msg in &req.messages {
        if let ChatMessage::Assistant(m) = msg {
            if let Some(ref tcs) = m.tool_calls {
                for tc in tcs {
                    tool_id_to_name.insert(tc.id.clone(), tc.function.name.clone());
                }
            }
        }
    }

    for msg in &req.messages {
        match msg {
            ChatMessage::System(m) => {
                let txt = m.content.as_plain_text();
                if !txt.trim().is_empty() {
                    system_instruction_parts.push(json!({"text": txt}));
                }
            }
            ChatMessage::Developer(m) => {
                let txt = m.content.as_plain_text();
                if !txt.trim().is_empty() {
                    system_instruction_parts.push(json!({"text": txt}));
                }
            }
            ChatMessage::User(m) => {
                let mut parts = Vec::new();
                match &m.content {
                    crate::openai::chat::MessageContent::Text(txt) => {
                        if first_user_text.is_none() && !txt.trim().is_empty() {
                            first_user_text = Some(txt.clone());
                        }
                        if !txt.trim().is_empty() {
                            parts.push(json!({"text": txt}));
                        }
                    }
                    crate::openai::chat::MessageContent::Parts(c_parts) => {
                        for p in c_parts {
                            match p {
                                crate::openai::chat::ContentPart::Text { text } => {
                                    if first_user_text.is_none() && !text.trim().is_empty() {
                                        first_user_text = Some(text.clone());
                                    }
                                    if !text.trim().is_empty() {
                                        parts.push(json!({"text": text}));
                                    }
                                }
                                crate::openai::chat::ContentPart::ImageUrl { image_url } => {
                                    if first_user_text.is_none() {
                                        first_user_text = Some(format!("img_seed_{}", &image_url.url[..image_url.url.len().min(40)]));
                                    }
                                    if let Some(inline) = parse_inline_data_part(&image_url.url, "image/jpeg") {
                                        parts.push(inline);
                                    }
                                }
                                crate::openai::chat::ContentPart::InputAudio { input_audio } => {
                                    if first_user_text.is_none() {
                                        first_user_text = Some(format!("aud_seed_{}", &input_audio.data[..input_audio.data.len().min(40)]));
                                    }
                                    let mime = map_audio_format_to_mime(&input_audio.format);
                                    if let Some(inline) = parse_inline_data_part(&input_audio.data, mime) {
                                        parts.push(inline);
                                    }
                                }
                                crate::openai::chat::ContentPart::VideoUrl { video_url } => {
                                    if first_user_text.is_none() {
                                        first_user_text = Some(format!("vid_seed_{}", &video_url.url[..video_url.url.len().min(40)]));
                                    }
                                    if let Some(inline) = parse_inline_data_part(&video_url.url, "video/mp4") {
                                        parts.push(inline);
                                    }
                                }
                                crate::openai::chat::ContentPart::File { file } => {
                                    if first_user_text.is_none() {
                                        first_user_text = Some(file.filename.clone().unwrap_or_else(|| "file_seed".to_string()));
                                    }
                                    if let Some(ref url) = file.file_url {
                                        if let Some(inline) = parse_inline_data_part(url, "application/pdf") {
                                            parts.push(inline);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if !parts.is_empty() {
                    push_or_merge_turn(&mut contents, "user", parts);
                }
            }
            ChatMessage::Assistant(m) => {
                let mut parts = Vec::new();
                let txt = m.content.as_ref().map(|c| c.as_plain_text()).unwrap_or_default();
                if !txt.trim().is_empty() {
                    parts.push(json!({"text": txt}));
                }
                if let Some(ref tool_calls) = m.tool_calls {
                    for tc in tool_calls {
                        let args_val: Value = serde_json::from_str(&tc.function.arguments)
                            .unwrap_or_else(|_| json!({}));
                        parts.push(json!({
                            "functionCall": {
                                "name": tc.function.name,
                                "args": args_val
                            },
                            "thoughtSignature": "skip_thought_signature_validator"
                        }));
                    }
                }
                push_or_merge_turn(&mut contents, "model", parts);
            }
            ChatMessage::Tool(m) => {
                let func_name = tool_id_to_name.get(&m.tool_call_id).cloned().unwrap_or_else(|| "tool".to_string());
                let mut extra_inline_parts = Vec::new();
                let txt = match &m.content {
                    crate::openai::chat::MessageContent::Text(t) => t.clone(),
                    crate::openai::chat::MessageContent::Parts(c_parts) => {
                        let mut text_acc = Vec::new();
                        for part in c_parts {
                            match part {
                                crate::openai::chat::ContentPart::Text { text } => {
                                    text_acc.push(text.clone());
                                }
                                crate::openai::chat::ContentPart::ImageUrl { image_url } => {
                                    if let Some(inline) = parse_inline_data_part(&image_url.url, "image/jpeg") {
                                        extra_inline_parts.push(inline);
                                    }
                                }
                                crate::openai::chat::ContentPart::InputAudio { input_audio } => {
                                    let mime = map_audio_format_to_mime(&input_audio.format);
                                    if let Some(inline) = parse_inline_data_part(&input_audio.data, mime) {
                                        extra_inline_parts.push(inline);
                                    }
                                }
                                crate::openai::chat::ContentPart::VideoUrl { video_url } => {
                                    if let Some(inline) = parse_inline_data_part(&video_url.url, "video/mp4") {
                                        extra_inline_parts.push(inline);
                                    }
                                }
                                crate::openai::chat::ContentPart::File { file } => {
                                    if let Some(ref url) = file.file_url {
                                        if let Some(inline) = parse_inline_data_part(url, "application/pdf") {
                                            extra_inline_parts.push(inline);
                                        }
                                    }
                                }
                            }
                        }
                        text_acc.join("\n")
                    }
                };
                let response_obj = match serde_json::from_str::<Value>(&txt) {
                    Ok(Value::Object(map)) => Value::Object(map),
                    Ok(v) => json!({"response": v}),
                    Err(_) => json!({"response": txt}),
                };
                let mut user_turn_parts = vec![json!({
                    "functionResponse": {
                        "name": func_name,
                        "response": response_obj
                    }
                })];
                // Parallel inline data for Gemini Tool Calling contract
                user_turn_parts.extend(extra_inline_parts);
                push_or_merge_turn(&mut contents, "user", user_turn_parts);
            }
            ChatMessage::Function(m) => {
                let txt = m.content.clone().unwrap_or_default();
                let response_obj = match serde_json::from_str::<Value>(&txt) {
                    Ok(Value::Object(map)) => Value::Object(map),
                    Ok(v) => json!({"response": v}),
                    Err(_) => json!({"response": txt}),
                };
                push_or_merge_turn(&mut contents, "user", vec![json!({
                    "functionResponse": {
                        "name": m.name,
                        "response": response_obj
                    }
                })]);
            }
        }
    }

    let session_id = extract_or_generate_session_id(first_user_text.as_deref(), session_salt);
    let trajectory_id = Uuid::new_v4().to_string();
    let request_id = generate_antigravity_request_id(&trajectory_id, 1);
    let used_claude = model.to_ascii_lowercase().contains("claude");

    let mut inner_request = json!({
        "contents": contents,
        "sessionId": session_id,
        "labels": {
            "last_step_index": "1",
            "model_enum": model,
            "trajectory_id": trajectory_id,
            "used_claude": if used_claude { "true" } else { "false" },
            "used_claude_conservative": if used_claude { "true" } else { "false" }
        },
        "toolConfig": {
            "functionCallingConfig": {
                "mode": "VALIDATED"
            }
        },
        "safetySettings": permissive_safety_settings()
    });

    if let Some(ref tools_def) = req.tools {
        if let Some(gemini_tools) = convert_tools_to_gemini(tools_def) {
            inner_request["tools"] = gemini_tools;
        }
    }

    if !system_instruction_parts.is_empty() {
        inner_request["systemInstruction"] = json!({
            "parts": system_instruction_parts
        });
    }

    let mut gen_config = json!({});
    if let Some(t) = req.temperature {
        gen_config["temperature"] = json!(t);
    }
    if let Some(p) = req.top_p {
        gen_config["topP"] = json!(p);
    }
    if let Some(m) = req.max_tokens.or(req.max_completion_tokens) {
        gen_config["maxOutputTokens"] = json!(m);
    }
    if let Some(thinking_cfg) = antigravity_thinking_config(model, thinking) {
        clamp_max_output_for_thinking_budget(&mut gen_config, &thinking_cfg, model, thinking);
        if gen_config.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
            inner_request["generationConfig"] = gen_config;
        }
        if !inner_request
            .get("generationConfig")
            .is_some_and(|v| v.is_object())
        {
            inner_request["generationConfig"] = json!({});
        }
        inner_request["generationConfig"]["thinkingConfig"] = thinking_cfg;
    } else if gen_config.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
        inner_request["generationConfig"] = gen_config;
    }

    let envelope = json!({
        "project": project_id,
        "requestId": request_id.clone(),
        "request": inner_request,
        "model": model,
        "userAgent": "antigravity",
        "requestType": "agent"
    });

    tracing::debug!(
        target_model = %model,
        request_id = %request_id,
        session_id = %session_id,
        messages_count = req.messages.len(),
        has_system = !system_instruction_parts.is_empty(),
        max_output_tokens = ?envelope["request"]["generationConfig"].get("maxOutputTokens"),
        thinking_config = ?envelope["request"]["generationConfig"].get("thinkingConfig"),
        "Constructed Antigravity chat request envelope"
    );

    Ok(envelope)
}

/// Convert Anthropic MessageRequest into Antigravity CLI envelope
/// (`thinking` semantics identical to [`chat_to_antigravity_request`]).
pub fn messages_to_antigravity_request(
    req: &MessageRequest,
    model: &str,
    project_id: &str,
    thinking: Option<ReasoningEffort>,
    session_salt: &str,
) -> Result<Value> {
    let mut contents = Vec::new();
    let mut first_user_text: Option<String> = None;
    let mut system_instruction_parts = Vec::new();
    let mut tool_id_to_name: HashMap<String, String> = HashMap::new();

    if let Some(ref sys) = req.system {
        let sys_text = sys.as_plain_text();
        if !sys_text.trim().is_empty() {
            system_instruction_parts.push(json!({"text": sys_text}));
        }
    }

    for msg in &req.messages {
        if let crate::anthropic::messages::AnthropicContent::Blocks(blocks) = &msg.content {
            for b in blocks {
                if let crate::anthropic::messages::AnthropicContentBlock::ToolUse { id, name, .. } = b {
                    tool_id_to_name.insert(id.clone(), name.clone());
                }
            }
        }
    }

    for msg in &req.messages {
        let is_model = msg.role == crate::anthropic::messages::AnthropicRole::Assistant;
        let role_str = if is_model { "model" } else { "user" };
        let mut parts = Vec::new();

        match &msg.content {
            crate::anthropic::messages::AnthropicContent::Text(t) => {
                if !t.trim().is_empty() {
                    if first_user_text.is_none() && role_str == "user" {
                        first_user_text = Some(t.clone());
                    }
                    parts.push(json!({"text": t}));
                }
            }
            crate::anthropic::messages::AnthropicContent::Blocks(blocks) => {
                for b in blocks {
                    match b {
                        crate::anthropic::messages::AnthropicContentBlock::Text { text, .. } => {
                            if !text.trim().is_empty() {
                                if first_user_text.is_none() && role_str == "user" {
                                    first_user_text = Some(text.clone());
                                }
                                parts.push(json!({"text": text}));
                            }
                        }
                        crate::anthropic::messages::AnthropicContentBlock::ToolUse { name, input, .. } => {
                            parts.push(json!({
                                "functionCall": {
                                    "name": name,
                                    "args": input
                                },
                                "thoughtSignature": "skip_thought_signature_validator"
                            }));
                        }
                        crate::anthropic::messages::AnthropicContentBlock::ToolResult { tool_use_id, content, .. } => {
                            let func_name = tool_id_to_name.get(tool_use_id).cloned().unwrap_or_else(|| "tool".to_string());
                            let mut extra_inline_parts = Vec::new();
                            let txt = match content {
                                crate::anthropic::messages::ToolResultContent::Text(s) => s.clone(),
                                crate::anthropic::messages::ToolResultContent::Blocks(bls) => {
                                    bls.iter().filter_map(|blk| match blk {
                                        crate::anthropic::messages::ToolResultBlock::Text { text } => Some(text.clone()),
                                        crate::anthropic::messages::ToolResultBlock::Image { source } => {
                                            if let Some(inline) = parse_inline_data_part(&format!("data:{};base64,{}", source.media_type, source.data), &source.media_type) {
                                                extra_inline_parts.push(inline);
                                            }
                                            None
                                        }
                                    }).collect::<Vec<_>>().join("\n")
                                }
                            };
                            let response_obj = match serde_json::from_str::<Value>(&txt) {
                                Ok(Value::Object(map)) => Value::Object(map),
                                Ok(v) => json!({"response": v}),
                                Err(_) => json!({"response": txt}),
                            };
                            parts.push(json!({
                                "functionResponse": {
                                    "name": func_name,
                                    "response": response_obj
                                }
                            }));
                            // Parallel inline data for Gemini Tool Calling contract
                            parts.extend(extra_inline_parts);
                        }
                        crate::anthropic::messages::AnthropicContentBlock::Image { source, .. } => {
                            if first_user_text.is_none() && role_str == "user" {
                                first_user_text = Some(format!("anthropic_img_{}", &source.data[..source.data.len().min(40)]));
                            }
                            if let Some(inline) = parse_inline_data_part(
                                &format!("data:{};base64,{}", source.media_type, source.data),
                                &source.media_type,
                            ) {
                                parts.push(inline);
                            }
                        }
                        crate::anthropic::messages::AnthropicContentBlock::Document { source, .. } => {
                            if first_user_text.is_none() && role_str == "user" {
                                first_user_text = Some(format!("anthropic_doc_{}", &source.data[..source.data.len().min(40)]));
                            }
                            if let Some(inline) = parse_inline_data_part(
                                &format!("data:{};base64,{}", source.media_type, source.data),
                                &source.media_type,
                            ) {
                                parts.push(inline);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        push_or_merge_turn(&mut contents, role_str, parts);
    }

    let session_id = extract_or_generate_session_id(first_user_text.as_deref(), session_salt);
    let trajectory_id = Uuid::new_v4().to_string();
    let request_id = generate_antigravity_request_id(&trajectory_id, 1);
    let used_claude = model.to_ascii_lowercase().contains("claude");

    let mut inner_request = json!({
        "contents": contents,
        "sessionId": session_id,
        "labels": {
            "last_step_index": "1",
            "model_enum": model,
            "trajectory_id": trajectory_id,
            "used_claude": if used_claude { "true" } else { "false" },
            "used_claude_conservative": if used_claude { "true" } else { "false" }
        },
        "toolConfig": {
            "functionCallingConfig": {
                "mode": "VALIDATED"
            }
        },
        "safetySettings": permissive_safety_settings()
    });

    if let Some(ref tools_def) = req.tools {
        if let Some(gemini_tools) = convert_anthropic_tools_to_gemini(tools_def) {
            inner_request["tools"] = gemini_tools;
        }
    }

    if !system_instruction_parts.is_empty() {
        inner_request["systemInstruction"] = json!({
            "parts": system_instruction_parts
        });
    }

    let mut gen_config = json!({
        "maxOutputTokens": req.max_tokens
    });
    if let Some(t) = req.temperature {
        gen_config["temperature"] = json!(t);
    }
    if let Some(p) = req.top_p {
        gen_config["topP"] = json!(p);
    }
    if let Some(thinking_cfg) = antigravity_thinking_config(model, thinking) {
        clamp_max_output_for_thinking_budget(&mut gen_config, &thinking_cfg, model, thinking);
        gen_config["thinkingConfig"] = thinking_cfg;
    }
    inner_request["generationConfig"] = gen_config;

    let envelope = json!({
        "project": project_id,
        "requestId": request_id.clone(),
        "request": inner_request,
        "model": model,
        "userAgent": "antigravity",
        "requestType": "agent"
    });

    tracing::debug!(
        target_model = %model,
        request_id = %request_id,
        session_id = %session_id,
        messages_count = req.messages.len(),
        has_system = !system_instruction_parts.is_empty(),
        max_output_tokens = ?envelope["request"]["generationConfig"].get("maxOutputTokens"),
        thinking_config = ?envelope["request"]["generationConfig"].get("thinkingConfig"),
        "Constructed Antigravity messages request envelope"
    );

    Ok(envelope)
}

/// Convert Antigravity response JSON to OpenAI ChatCompletion response Value
pub fn antigravity_to_chat_response(
    resp: &Value,
    model: &str,
) -> Value {
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Accept both the collected shape (`candidates` at top) and raw upstream
    // frames (`response.candidates` envelope).
    let target = resp.get("response").unwrap_or(resp);
    let mut full_text = String::new();
    let mut reasoning_text = String::new();
    let mut tool_calls = Vec::new();
    let mut finish_reason = "stop";

    if let Some(candidates) = target.get("candidates").and_then(|v| v.as_array()) {
        if let Some(first) = candidates.first() {
            if let Some(f_reason) = first.get("finishReason").and_then(|v| v.as_str()) {
                finish_reason = match f_reason {
                    "MAX_TOKENS" => "length",
                    "SAFETY" => "content_filter",
                    _ => "stop",
                };
            }
            if let Some(parts) = first.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                for p in parts {
                    let is_thought = p.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
                    if let Some(txt) = p.get("text").and_then(|t| t.as_str()) {
                        if is_thought {
                            reasoning_text.push_str(txt);
                        } else {
                            full_text.push_str(txt);
                        }
                    }
                    if let Some(fc) = p.get("functionCall") {
                        let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string();
                        let args_str = match fc.get("args") {
                            Some(Value::String(s)) => s.clone(),
                            Some(v) => serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()),
                            None => "{}".to_string(),
                        };
                        let call_id = format!("call_{}", Uuid::new_v4().simple());
                        tool_calls.push(json!({
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": args_str
                            }
                        }));
                    }
                }
            }
        }
    }

    if !tool_calls.is_empty() && finish_reason == "stop" {
        finish_reason = "tool_calls";
    }

    let prompt_tokens = target.get("usageMetadata")
        .and_then(|u| u.get("promptTokenCount"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);
    let completion_tokens = target.get("usageMetadata")
        .and_then(|u| u.get("candidatesTokenCount"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);
    let total_tokens = target.get("usageMetadata")
        .and_then(|u| u.get("totalTokenCount"))
        .and_then(|t| t.as_u64())
        .unwrap_or(prompt_tokens + completion_tokens);

    let mut message = json!({
        "role": "assistant",
        "content": if full_text.is_empty() && !tool_calls.is_empty() { Value::Null } else { json!(full_text) }
    });
    if !reasoning_text.is_empty() {
        message["reasoning_content"] = json!(reasoning_text);
    }
    if !tool_calls.is_empty() {
        message["tool_calls"] = json!(tool_calls);
    }

    if full_text.is_empty() && tool_calls.is_empty() {
        tracing::warn!(
            target_model = %model,
            finish_reason = %finish_reason,
            thought_len = reasoning_text.len(),
            prompt_tokens,
            completion_tokens,
            "Antigravity non-stream response converted with empty text content (potential token limit exhaustion or unhandled tool_call)"
        );
    } else {
        tracing::debug!(
            target_model = %model,
            finish_reason = %finish_reason,
            thought_len = reasoning_text.len(),
            content_len = full_text.len(),
            prompt_tokens,
            completion_tokens,
            "Antigravity non-stream response successfully converted to ChatCompletion"
        );
    }

    let cached_tokens = target.get("usageMetadata")
        .and_then(|u| u.get("cachedContentTokenCount"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);

    let mut usage_json = json!({
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "total_tokens": total_tokens
    });
    if cached_tokens > 0 {
        usage_json["prompt_tokens_details"] = json!({
            "cached_tokens": cached_tokens
        });
    }

    json!({
        "id": format!("chatcmpl-{}", Uuid::new_v4().simple()),
        "object": "chat.completion",
        "created": now_ts,
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": message,
                "finish_reason": finish_reason
            }
        ],
        "usage": usage_json
    })
}

/// Convert Antigravity response JSON to Anthropic Message response Value
pub fn antigravity_to_messages_response(
    resp: &Value,
    model: &str,
) -> Value {
    let target = resp.get("response").unwrap_or(resp);
    let mut full_text = String::new();
    let mut reasoning_text = String::new();
    let mut tool_uses = Vec::new();
    let mut stop_reason = "end_turn";

    if let Some(candidates) = target.get("candidates").and_then(|v| v.as_array()) {
        if let Some(first) = candidates.first() {
            if let Some(f_reason) = first.get("finishReason").and_then(|v| v.as_str()) {
                stop_reason = match f_reason {
                    "MAX_TOKENS" => "max_tokens",
                    "SAFETY" => "stop_sequence",
                    _ => "end_turn",
                };
            }
            if let Some(parts) = first.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                for p in parts {
                    let is_thought = p.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
                    if let Some(txt) = p.get("text").and_then(|t| t.as_str()) {
                        if is_thought {
                            reasoning_text.push_str(txt);
                        } else {
                            full_text.push_str(txt);
                        }
                    }
                    if let Some(fc) = p.get("functionCall") {
                        let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string();
                        let args_val = match fc.get("args") {
                            Some(v) => v.clone(),
                            None => json!({}),
                        };
                        let call_id = format!("toolu_{}", Uuid::new_v4().simple());
                        tool_uses.push(json!({
                            "type": "tool_use",
                            "id": call_id,
                            "name": name,
                            "input": args_val
                        }));
                    }
                }
            }

            if !tool_uses.is_empty() && stop_reason == "end_turn" {
                stop_reason = "tool_use";
            }
        }
    }

    let prompt_tokens = target.get("usageMetadata")
        .and_then(|u| u.get("promptTokenCount"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);
    let completion_tokens = target.get("usageMetadata")
        .and_then(|u| u.get("candidatesTokenCount"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);

    let mut content_blocks = Vec::new();
    if !reasoning_text.is_empty() {
        content_blocks.push(json!({
            "type": "thinking",
            "thinking": reasoning_text
        }));
    }
    if !full_text.is_empty() {
        content_blocks.push(json!({
            "type": "text",
            "text": full_text
        }));
    }
    for tu in tool_uses {
        content_blocks.push(tu);
    }

    if content_blocks.is_empty() {
        tracing::warn!(
            target_model = %model,
            stop_reason = %stop_reason,
            thought_len = reasoning_text.len(),
            input_tokens = prompt_tokens,
            output_tokens = completion_tokens,
            "Antigravity non-stream response converted with empty text content for Anthropic messages (potential token limit exhaustion or unhandled tool_call)"
        );
    } else {
        tracing::debug!(
            target_model = %model,
            stop_reason = %stop_reason,
            thought_len = reasoning_text.len(),
            content_len = full_text.len(),
            input_tokens = prompt_tokens,
            output_tokens = completion_tokens,
            "Antigravity non-stream response successfully converted to Anthropic Message"
        );
    }

    let cached_tokens = target.get("usageMetadata")
        .and_then(|u| u.get("cachedContentTokenCount"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);

    let mut usage_json = json!({
        "input_tokens": prompt_tokens,
        "output_tokens": completion_tokens
    });
    if cached_tokens > 0 {
        usage_json["cache_read_input_tokens"] = json!(cached_tokens);
    }

    json!({
        "id": format!("msg_{}", Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content_blocks,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": usage_json
    })
}

/// Convert an SSE line/chunk from Antigravity into OpenAI SSE chunk
pub fn antigravity_chunk_to_chat_chunk(
    chunk_json: &Value,
    model: &str,
    response_id: &str,
) -> Option<ChatCompletionChunk> {
    // Upstream SSE frames wrap the Gemini payload in a `response` envelope:
    // `data: {"response": {"candidates": [...]}}`. The non-stream collector
    // already unwraps this; the streaming path must do the same, otherwise
    // every chunk yields None and the client sees only the terminal frame.
    let target = chunk_json.get("response").unwrap_or(chunk_json);
    let candidates = target.get("candidates")?.as_array()?;
    let first = candidates.first()?;
    
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();
    if let Some(parts) = first.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
        for (idx, p) in parts.iter().enumerate() {
            let is_thought = p.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
            if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                if is_thought {
                    reasoning.push_str(t);
                } else {
                    text.push_str(t);
                }
            }
            if let Some(fc) = p.get("functionCall") {
                let name = fc.get("name").and_then(|n| n.as_str()).map(|s| s.to_string());
                let args = match fc.get("args") {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(v) => Some(serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string())),
                    None => None,
                };
                let id = format!("call_{}", Uuid::new_v4().simple());
                tool_calls.push(crate::openai::chat::ToolCallChunk {
                    index: idx as u32,
                    id: Some(id),
                    r#type: Some("function".to_string()),
                    function: Some(crate::openai::chat::FunctionCallChunk {
                        name,
                        arguments: args,
                    }),
                });
            }
        }
    }

    let has_tools = !tool_calls.is_empty();
    let finish_reason = first.get("finishReason").and_then(|v| v.as_str()).map(|r| {
        match r {
            "MAX_TOKENS" => FinishReason::Length,
            "SAFETY" => FinishReason::ContentFilter,
            _ => {
                if has_tools {
                    FinishReason::ToolCalls
                } else {
                    FinishReason::Stop
                }
            }
        }
    });

    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let delta = ChatChunkDelta {
        role: None,
        content: if text.is_empty() { None } else { Some(text) },
        reasoning_content: if reasoning.is_empty() { None } else { Some(reasoning) },
        refusal: None,
        tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
    };

    let usage = target.get("usageMetadata").map(|u| {
        let prompt_tokens = u.get("promptTokenCount").and_then(|t| t.as_u64()).unwrap_or(0) as u32;
        let completion_tokens = u.get("candidatesTokenCount").and_then(|t| t.as_u64()).unwrap_or(0) as u32;
        let total_tokens = u.get("totalTokenCount").and_then(|t| t.as_u64()).unwrap_or((prompt_tokens + completion_tokens) as u64) as u32;
        let cached_tokens = u.get("cachedContentTokenCount").and_then(|t| t.as_u64()).map(|c| c as u32);
        let prompt_tokens_details = cached_tokens.map(|c| crate::openai::chat::PromptTokensDetails {
            cached_tokens: Some(c),
            audio_tokens: None,
        });
        Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            prompt_tokens_details,
            completion_tokens_details: None,
        }
    });

    Some(ChatCompletionChunk {
        id: response_id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created: now_ts,
        model: model.to_string(),
        choices: vec![ChatChunkChoice {
            index: 0,
            delta,
            finish_reason,
            logprobs: None,
        }],
        usage,
        system_fingerprint: None,
        service_tier: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai::chat::SystemMessage;

    #[test]
    fn test_trajectory_id_aligned_between_request_id_and_labels() {
        // P0-5: requestId's trajectory segment and labels.trajectory_id
        // must be the same id; sessionId stays an independent value.
        let mut req = ChatCompletionRequest::default();
        req.model = "claude-sonnet-4-6".to_string();
        req.messages.push(ChatMessage::User(crate::openai::chat::UserMessage {
            content: "Hello!".into(),
            name: None,
        }));

        let env = chat_to_antigravity_request(&req, "claude-sonnet-4-6", "proj-1", None, "").unwrap();
        let request_id = env["requestId"].as_str().unwrap();
        // agent/{uuid}/{ms}/{trajectory}/{step}
        let segs: Vec<&str> = request_id.split('/').collect();
        assert_eq!((segs[0], segs.len()), ("agent", 5), "got {}", request_id);
        let traj_in_id = segs[3];
        assert_eq!(env["request"]["labels"]["trajectory_id"].as_str().unwrap(), traj_in_id);
        // sessionId is a distinct value (prompt hash), not the trajectory.
        assert_ne!(env["request"]["sessionId"].as_str().unwrap(), traj_in_id);
    }

    #[test]
    fn test_session_id_isolated_by_salt() {
        // B7: identical prompts under different key salts must not share
        // a sessionId; empty salt preserves the legacy digest.
        let a = extract_or_generate_session_id(Some("Count from 1 to 5."), "key-1");
        let b = extract_or_generate_session_id(Some("Count from 1 to 5."), "key-2");
        let legacy = extract_or_generate_session_id(Some("Count from 1 to 5."), "");
        assert_ne!(a, b);
        assert_ne!(a, legacy);
        assert_ne!(b, legacy);
    }

    #[test]
    fn test_chat_to_antigravity_envelope() {
        let mut req = ChatCompletionRequest::default();
        req.model = "gemini-3.8-flash-low".to_string();
        req.messages.push(ChatMessage::System(SystemMessage {
            content: "You are a helpful assistant.".into(),
            name: None,
        }));
        req.messages.push(ChatMessage::User(crate::openai::chat::UserMessage {
            content: "Hello!".into(),
            name: None,
        }));

        let env = chat_to_antigravity_request(&req, "gemini-3.8-flash-low", "aicode-consumers", None, "").unwrap();
        assert_eq!(env["project"], "aicode-consumers");
        assert_eq!(env["model"], "gemini-3.8-flash-low");
        assert_eq!(env["userAgent"], "antigravity");
        assert_eq!(env["requestType"], "agent");
        assert!(env["requestId"].as_str().unwrap().starts_with("agent/"));
        assert!(env["request"]["toolConfig"]["functionCallingConfig"]["mode"] == "VALIDATED");
        // No explicit effort → legacy wire shape, no thinkingConfig injected.
        assert!(env["request"].get("generationConfig").is_none());
    }

    #[test]
    fn test_antigravity_thinking_config_mapping() {
        // Legacy: no explicit effort → untouched wire shape.
        assert!(antigravity_thinking_config("gemini-2.5-flash", None).is_none());

        // Off suppresses thought return with thinkingBudget: 0 for gemini-2.x/claude.
        let off = antigravity_thinking_config("gemini-2.5-flash", Some(ReasoningEffort::Off)).unwrap();
        assert_eq!(off["includeThoughts"], false);
        assert_eq!(off["thinkingBudget"], 0);

        let off_g3 = antigravity_thinking_config("gemini-3.8-flash-low", Some(ReasoningEffort::Off)).unwrap();
        assert_eq!(off_g3["includeThoughts"], false);
        assert!(off_g3.get("thinkingBudget").is_none());

        // Tiered budgets on budget-honoring models.
        let low = antigravity_thinking_config("gemini-2.5-flash", Some(ReasoningEffort::Low)).unwrap();
        assert_eq!(low["thinkingBudget"], 1024);
        let med = antigravity_thinking_config("gemini-2.5-flash", Some(ReasoningEffort::Medium)).unwrap();
        assert_eq!(med["thinkingBudget"], 4096);
        let high = antigravity_thinking_config("claude-sonnet-4-6", Some(ReasoningEffort::High)).unwrap();
        assert_eq!(high["thinkingBudget"], 16384);
        assert_eq!(high["includeThoughts"], true);

        // gemini-3.x: route selects depth, budget must not be sent.
        for model in ["gemini-3.8-flash-high", "gemini-3.8-flash-low", "GEMINI-3.1-PRO-HIGH"] {
            let cfg = antigravity_thinking_config(model, Some(ReasoningEffort::High)).unwrap();
            assert_eq!(cfg["includeThoughts"], true);
            assert!(cfg.get("thinkingBudget").is_none(), "model {}", model);
            assert!(cfg.get("thinkingLevel").is_none(), "model {}", model);
        }
    }

    #[test]
    fn test_chat_to_antigravity_injects_thinking_config() {
        let mut req = ChatCompletionRequest::default();
        req.model = "gemini-2.5-flash".to_string();
        req.messages.push(ChatMessage::User(crate::openai::chat::UserMessage {
            content: "Hi".into(),
            name: None,
        }));

        let env = chat_to_antigravity_request(&req, "gemini-2.5-flash", "aicode-consumers", Some(ReasoningEffort::High), "").unwrap();
        assert_eq!(env["request"]["generationConfig"]["thinkingConfig"]["thinkingBudget"], 16384);
    }

    #[test]
    fn test_thinking_budget_raises_small_max_output_cap() {
        use crate::anthropic::messages::{AnthropicContent, AnthropicMessage, AnthropicRole};
        let req = MessageRequest {
            model: "gemini-2.5-flash".to_string(),
            messages: vec![AnthropicMessage {
                role: AnthropicRole::User,
                content: AnthropicContent::Text("Hi".to_string()),
            }],
            max_tokens: 500,
            system: None,
            metadata: None,
            stop_sequences: None,
            stream: None,
            temperature: None,
            top_p: None,
            top_k: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            reasoning_effort: None,
            extra: Default::default(),
        };

        // High budget (16384) exceeds the 500 cap → raised to budget + 1024 (17408) to satisfy maxOutputTokens > thinkingBudget.
        let env = messages_to_antigravity_request(&req, "gemini-2.5-flash", "aicode-consumers", Some(ReasoningEffort::High), "").unwrap();
        assert_eq!(env["request"]["generationConfig"]["maxOutputTokens"], 17408);

        // Low budget (1024) exceeds the 500 cap → raised to 2048.
        let env = messages_to_antigravity_request(&req, "gemini-2.5-flash", "aicode-consumers", Some(ReasoningEffort::Low), "").unwrap();
        assert_eq!(env["request"]["generationConfig"]["maxOutputTokens"], 2048);

        // No thinking → caller's cap preserved verbatim (legacy).
        let env = messages_to_antigravity_request(&req, "gemini-2.5-flash", "aicode-consumers", None, "").unwrap();
        assert_eq!(env["request"]["generationConfig"]["maxOutputTokens"], 500);
        assert!(env["request"]["generationConfig"].get("thinkingConfig").is_none());
    }

    #[test]
    fn test_gemini_thinking_off_budget_zero() {
        let cfg = antigravity_thinking_config("gemini-2.5-flash", Some(ReasoningEffort::Off)).unwrap();
        assert_eq!(cfg["includeThoughts"], false);
        assert_eq!(cfg["thinkingBudget"], 0);

        let claude_cfg = antigravity_thinking_config("claude-sonnet-4-6", Some(ReasoningEffort::Off)).unwrap();
        assert_eq!(claude_cfg["includeThoughts"], false);
        assert_eq!(claude_cfg["thinkingBudget"], 0);
    }

    #[test]
    fn test_claude_thinking_budget_clamping_strict_greater() {
        let mut gen_cfg = json!({
            "maxOutputTokens": 1024
        });
        let thinking_cfg = json!({
            "thinkingBudget": 1024
        });
        clamp_max_output_for_thinking_budget(&mut gen_cfg, &thinking_cfg, "claude-sonnet-4-6", Some(ReasoningEffort::Low));
        assert_eq!(gen_cfg["maxOutputTokens"], 2048);
    }

    #[test]
    fn test_thought_content_separated_to_reasoning_content() {
        let ant_resp = json!({
            "candidates": [
                {
                    "content": {
                        "role": "model",
                        "parts": [
                            {
                                "thought": true,
                                "text": "Analyzing the question..."
                            },
                            {
                                "text": "Here is the direct answer."
                            }
                        ]
                    },
                    "finishReason": "STOP"
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 25,
                "totalTokenCount": 35
            }
        });

        // Chat response separation
        let chat_resp = antigravity_to_chat_response(&ant_resp, "gemini-2.5-flash");
        assert_eq!(chat_resp["choices"][0]["message"]["content"], "Here is the direct answer.");
        assert_eq!(chat_resp["choices"][0]["message"]["reasoning_content"], "Analyzing the question...");

        // Messages response separation
        let msg_resp = antigravity_to_messages_response(&ant_resp, "claude-sonnet-4-6");
        let content_blocks = msg_resp["content"].as_array().unwrap();
        assert_eq!(content_blocks.len(), 2);
        assert_eq!(content_blocks[0]["type"], "thinking");
        assert_eq!(content_blocks[0]["thinking"], "Analyzing the question...");
        assert_eq!(content_blocks[1]["type"], "text");
        assert_eq!(content_blocks[1]["text"], "Here is the direct answer.");
    }

    #[test]
    fn test_antigravity_to_chat_response() {
        let ant_resp = json!({
            "candidates": [
                {
                    "content": {
                        "parts": [{"text": "Hello human!"}],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 5,
                "candidatesTokenCount": 10,
                "totalTokenCount": 15
            }
        });

        let chat_resp = antigravity_to_chat_response(&ant_resp, "gemini-3.8-flash-low");
        assert_eq!(chat_resp["choices"][0]["message"]["content"], "Hello human!");
        assert_eq!(chat_resp["choices"][0]["finish_reason"], "stop");
        assert_eq!(chat_resp["usage"]["total_tokens"], 15);
    }

    #[test]
    fn test_antigravity_chunk_to_chat_chunk() {
        let chunk_json = json!({
            "candidates": [
                {
                    "content": {
                        "parts": [{"text": "Stream chunk content"}]
                    },
                    "finishReason": "STOP"
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 8,
                "candidatesTokenCount": 4,
                "totalTokenCount": 12
            }
        });

        let chunk = antigravity_chunk_to_chat_chunk(&chunk_json, "gemini-3.8-flash-low", "chatcmpl-test-123").unwrap();
        assert_eq!(chunk.id, "chatcmpl-test-123");
        assert_eq!(chunk.model, "gemini-3.8-flash-low");
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Stream chunk content"));
        assert_eq!(chunk.choices[0].finish_reason, Some(FinishReason::Stop));
        assert_eq!(chunk.usage.as_ref().unwrap().total_tokens, 12);
    }

    #[test]
    fn test_antigravity_chunk_unwraps_response_envelope() {
        // Live upstream SSE frames nest the payload: `data: {"response": {...}}`.
        // The streaming translator must unwrap it (mirrors collect_antigravity_sse_to_json).
        let enveloped = json!({
            "response": {
                "candidates": [
                    {
                        "content": {
                            "role": "model",
                            "parts": [{"text": "1,"}]
                        }
                    }
                ],
                "usageMetadata": {
                    "promptTokenCount": 16,
                    "candidatesTokenCount": 1,
                    "totalTokenCount": 17
                }
            }
        });

        let chunk = antigravity_chunk_to_chat_chunk(&enveloped, "claude-sonnet-4-6", "chatcmpl-env-1")
            .expect("enveloped chunk must translate");
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("1,"));
        assert_eq!(chunk.usage.as_ref().unwrap().total_tokens, 17);
    }
}
