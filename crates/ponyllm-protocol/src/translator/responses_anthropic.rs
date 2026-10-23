use crate::anthropic::messages::*;
use crate::common::ReasoningEffort;
use crate::error::{ProtocolError, Result};
use crate::openai::responses::*;
use crate::sanitize::{sanitize_extra, TargetProtocol};

use serde_json::json;

/// Merge consecutive same-role messages by concatenating their blocks so the
/// output always satisfies Anthropic's strict role alternation.
fn merge_consecutive_same_role(messages: Vec<AnthropicMessage>) -> Vec<AnthropicMessage> {
    let mut out: Vec<AnthropicMessage> = Vec::with_capacity(messages.len());
    for msg in messages {
        let mergeable = match (out.last(), &msg) {
            (Some(last), _) if last.role == msg.role => matches!(
                (&last.content, &msg.content),
                (AnthropicContent::Blocks(_), AnthropicContent::Blocks(_))
            ),
            _ => false,
        };
        if mergeable {
            if let (Some(last), AnthropicContent::Blocks(blocks)) = (out.last_mut(), msg.content) {
                if let AnthropicContent::Blocks(acc) = &mut last.content {
                    acc.extend(blocks);
                }
            }
        } else {
            out.push(msg);
        }
    }
    out
}

/// Convert OpenAI Responses request to Anthropic Messages request.
/// Assistant reasoning (`Thought`/`Reasoning` parts) becomes `Thinking` blocks;
/// `FunctionCall`/`FunctionResponse` items become `ToolUse`/`ToolResult` blocks.
pub fn responses_to_anthropic_request(req: &CreateResponseRequest) -> Result<MessageRequest> {
    let mut messages: Vec<AnthropicMessage> = Vec::new();

    let push_text = |messages: &mut Vec<AnthropicMessage>, role: AnthropicRole, text: String| {
        messages.push(AnthropicMessage {
            role,
            content: AnthropicContent::Text(text),
        });
    };

    match &req.input {
        ResponseInput::Text(t) => {
            if t.trim().is_empty() {
                return Err(crate::error::ProtocolError::Validation(
                    "no translatable content for Anthropic Messages: input carries no text, thinking, or tool blocks".to_string(),
                ));
            }
            push_text(&mut messages, AnthropicRole::User, t.clone());
        }
        ResponseInput::Items(items) => {
            let mut pending_use: Vec<AnthropicContentBlock> = Vec::new();
            let mut pending_result: Vec<AnthropicContentBlock> = Vec::new();
            let flush_use = |messages: &mut Vec<AnthropicMessage>,
                             pending: &mut Vec<AnthropicContentBlock>| {
                if !pending.is_empty() {
                    messages.push(AnthropicMessage {
                        role: AnthropicRole::Assistant,
                        content: AnthropicContent::Blocks(std::mem::take(pending)),
                    });
                }
            };
            let flush_result =
                |messages: &mut Vec<AnthropicMessage>, pending: &mut Vec<AnthropicContentBlock>| {
                    if !pending.is_empty() {
                        messages.push(AnthropicMessage {
                            role: AnthropicRole::User,
                            content: AnthropicContent::Blocks(std::mem::take(pending)),
                        });
                    }
                };
            for item in items {
                match item {
                    ResponseInputItem::Message { role, content } => {
                        flush_use(&mut messages, &mut pending_use);
                        flush_result(&mut messages, &mut pending_result);
                        let mut blocks = Vec::new();
                        let mut text_acc = String::new();
                        match content {
                            ResponseInputContent::Text(t) => {
                                text_acc.push_str(t);
                            }
                            ResponseInputContent::Parts(parts) => {
                                for part in parts {
                                    match part {
                                        ResponseContentPart::Text { text } => {
                                            text_acc.push_str(text);
                                        }
                                        ResponseContentPart::InputImage { image_url, .. } => {
                                            if !text_acc.is_empty() {
                                                blocks.push(AnthropicContentBlock::Text {
                                                    text: std::mem::take(&mut text_acc),
                                                    cache_control: None,
                                                });
                                            }
                                            let (media_type, data, src_type) = if image_url.starts_with("data:") {
                                                let rest = &image_url["data:".len()..];
                                                if let Some((mime, b64)) = rest.split_once(";base64,") {
                                                    (mime.to_string(), b64.to_string(), "base64".to_string())
                                                } else {
                                                    ("image/jpeg".to_string(), image_url.clone(), "url".to_string())
                                                }
                                            } else {
                                                ("image/jpeg".to_string(), image_url.clone(), "url".to_string())
                                            };
                                            blocks.push(AnthropicContentBlock::Image {
                                                source: AnthropicImageSource {
                                                    r#type: src_type,
                                                    media_type,
                                                    data,
                                                },
                                                cache_control: None,
                                            });
                                        }
                                        ResponseContentPart::InputFile { file_url, .. } => {
                                            if let Some(ref url) = file_url {
                                                if !text_acc.is_empty() {
                                                    blocks.push(AnthropicContentBlock::Text {
                                                        text: std::mem::take(&mut text_acc),
                                                        cache_control: None,
                                                    });
                                                }
                                                if url.starts_with("data:") {
                                                    let rest = &url["data:".len()..];
                                                    if let Some((mime, b64)) = rest.split_once(";base64,") {
                                                        blocks.push(AnthropicContentBlock::Document {
                                                            source: AnthropicDocumentSource {
                                                                r#type: "base64".to_string(),
                                                                media_type: mime.to_string(),
                                                                data: b64.to_string(),
                                                            },
                                                            cache_control: None,
                                                        });
                                                    } else {
                                                        blocks.push(AnthropicContentBlock::Text {
                                                            text: format!("[Document: {}]", url),
                                                            cache_control: None,
                                                        });
                                                    }
                                                } else {
                                                    blocks.push(AnthropicContentBlock::Text {
                                                        text: format!("[Document: {}]", url),
                                                        cache_control: None,
                                                    });
                                                }
                                            }
                                        }
                                        ResponseContentPart::InputVideo { video_url } => {
                                            if !text_acc.is_empty() {
                                                blocks.push(AnthropicContentBlock::Text {
                                                    text: std::mem::take(&mut text_acc),
                                                    cache_control: None,
                                                });
                                            }
                                            blocks.push(AnthropicContentBlock::Text {
                                                text: format!("[Video: {}]", video_url),
                                                cache_control: None,
                                            });
                                        }
                                        ResponseContentPart::InputAudio { .. } => {}
                                        ResponseContentPart::Thought { thought } => {
                                            blocks.push(AnthropicContentBlock::Thinking {
                                                thinking: thought.clone(),
                                                signature: None,
                                            });
                                        }
                                        ResponseContentPart::Reasoning { reasoning } => {
                                            blocks.push(AnthropicContentBlock::Thinking {
                                                thinking: reasoning.clone(),
                                                signature: None,
                                            });
                                        }
                                        ResponseContentPart::SummaryText { text }
                                        | ResponseContentPart::ReasoningText { text } => {
                                            blocks.push(AnthropicContentBlock::Thinking {
                                                thinking: text.clone(),
                                                signature: None,
                                            });
                                        }
                                        ResponseContentPart::Refusal { .. }
                                        | ResponseContentPart::Unknown => {}
                                    }
                                }
                            }
                        }
                        if !text_acc.is_empty() {
                            blocks.push(AnthropicContentBlock::Text {
                                text: text_acc,
                                cache_control: None,
                            });
                        }
                        let role = if role == "assistant" {
                            AnthropicRole::Assistant
                        } else {
                            AnthropicRole::User
                        };
                        // Empty content arrays are rejected by Anthropic upstreams;
                        // skip instead of emitting an illegal message.
                        if !blocks.is_empty() {
                            messages.push(AnthropicMessage {
                                role,
                                content: AnthropicContent::Blocks(blocks),
                            });
                        }
                    }
                    ResponseInputItem::FunctionCall {
                        call_id,
                        name,
                        arguments,
                    } => {
                        flush_result(&mut messages, &mut pending_result);
                        let input_val =
                            serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
                        pending_use.push(AnthropicContentBlock::ToolUse {
                            id: call_id.clone(),
                            name: crate::common::sanitize_wire_tool_name(name),
                            input: input_val,
                            cache_control: None,
                        });
                    }
                    ResponseInputItem::FunctionResponse { call_id, output } => {
                        flush_use(&mut messages, &mut pending_use);
                        pending_result.push(AnthropicContentBlock::ToolResult {
                            tool_use_id: call_id.clone(),
                            content: ToolResultContent::Text(output.clone()),
                            is_error: None,
                            cache_control: None,
                        });
                    }
                    ResponseInputItem::Reasoning { content, summary, .. } => {
                        let mut thought_texts = Vec::new();
                        if let Some(parts) = content {
                            for p in parts {
                                match p {
                                    ResponseContentPart::Thought { thought } => thought_texts.push(thought.clone()),
                                    ResponseContentPart::Reasoning { reasoning } => thought_texts.push(reasoning.clone()),
                                    ResponseContentPart::Text { text } => thought_texts.push(text.clone()),
                                    _ => {}
                                }
                            }
                        }
                        if let Some(parts) = summary {
                            for p in parts {
                                match p {
                                    ResponseContentPart::Thought { thought } => thought_texts.push(thought.clone()),
                                    ResponseContentPart::Reasoning { reasoning } => thought_texts.push(reasoning.clone()),
                                    ResponseContentPart::Text { text } => thought_texts.push(text.clone()),
                                    _ => {}
                                }
                            }
                        }
                        if !thought_texts.is_empty() {
                            flush_use(&mut messages, &mut pending_use);
                            flush_result(&mut messages, &mut pending_result);
                            messages.push(AnthropicMessage {
                                role: AnthropicRole::Assistant,
                                content: AnthropicContent::Blocks(vec![AnthropicContentBlock::Thinking {
                                    thinking: thought_texts.join("\n"),
                                    signature: None,
                                }]),
                            });
                        }
                    }
                    ResponseInputItem::ItemReference { .. } => {}
                    ResponseInputItem::Custom(_) => {}
                }
            }
            flush_use(&mut messages, &mut pending_use);
            flush_result(&mut messages, &mut pending_result);
            messages = merge_consecutive_same_role(messages);
            if messages.is_empty() {
                return Err(crate::error::ProtocolError::Validation(
                    "no translatable content for Anthropic Messages: input carries no text, thinking, or tool blocks".to_string(),
                ));
            }
        }
    }

    let tools = req.tools.as_ref().map(|t_list| {
        t_list
            .iter()
            .filter_map(|t| match t {
                ResponseToolDefinition::Function {
                    name,
                    description,
                    parameters,
                    ..
                } => Some(AnthropicTool {
                    name: crate::common::sanitize_wire_tool_name(name),
                    description: description.clone(),
                    input_schema: parameters
                        .clone()
                        .unwrap_or_else(|| json!({"type": "object"})),
                    cache_control: None,
                }),
                _ => None,
            })
            .collect()
    });

    let is_adaptive = ThinkingConfig::is_adaptive_model(&req.model);
    let (thinking, output_config, reasoning_effort) = match req.get_reasoning_effort() {
        Some(ReasoningEffort::Off) => {
            if is_adaptive {
                (None, None, Some(ReasoningEffort::Off))
            } else {
                (
                    Some(ThinkingConfig {
                        r#type: "disabled".to_string(),
                        budget_tokens: None,
                        effort: Some(ReasoningEffort::Off),
                    }),
                    None,
                    Some(ReasoningEffort::Off),
                )
            }
        }
        Some(effort) => {
            if is_adaptive {
                (
                    Some(ThinkingConfig {
                        r#type: "adaptive".to_string(),
                        budget_tokens: None,
                        effort: None,
                    }),
                    Some(crate::anthropic::messages::AnthropicOutputConfig {
                        effort: Some(effort),
                    }),
                    Some(effort),
                )
            } else {
                (
                    Some(ThinkingConfig {
                        r#type: "enabled".to_string(),
                        budget_tokens: None,
                        effort: Some(effort),
                    }),
                    None,
                    Some(effort),
                )
            }
        }
        None => (None, None, None),
    };

    Ok(MessageRequest {
        model: req.model.clone(),
        messages,
        max_tokens: req.max_output_tokens.unwrap_or(16384),
        system: req.instructions.clone().map(AnthropicSystem::Text),
        metadata: None,
        stop_sequences: None,
        stream: req.stream,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: None,
        tools,
        tool_choice: None,
        thinking,
        output_config,
        reasoning_effort,
        // Cross-protocol boundary (contract C1/C2): a client `extra` key must
        // never override the gateway-computed Anthropic fields.
        extra: sanitize_extra(req.extra.clone(), TargetProtocol::Anthropic),
    })
}


/// Convert Anthropic Messages request to OpenAI Responses request.
/// Assistant `Thinking` blocks become `Reasoning` parts; images are dropped
/// because Responses input items carry no image part.
pub fn anthropic_to_responses_request(req: &MessageRequest) -> Result<CreateResponseRequest> {
    let mut instructions = req.system.as_ref().map(|sys| match sys {
        AnthropicSystem::Text(t) => t.clone(),
        AnthropicSystem::Blocks(blocks) => blocks
            .iter()
            .map(|b| match b {
                AnthropicSystemBlock::Text { text, .. } => text.as_str(),
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
    });
    let mut items = Vec::new();

    for msg in &req.messages {
        match msg.role {
            AnthropicRole::Assistant => match &msg.content {
                AnthropicContent::Text(t) => {
                    items.push(ResponseInputItem::Message {
                        role: "assistant".to_string(),
                        content: ResponseInputContent::Text(t.clone()),
                    });
                }
                AnthropicContent::Blocks(blocks) => {
                    let mut parts = Vec::new();
                    for block in blocks {
                        match block {
                            AnthropicContentBlock::Text { text, .. } => {
                                parts.push(ResponseContentPart::Text { text: text.clone() });
                            }
                            AnthropicContentBlock::Thinking { thinking, .. } => {
                                parts.push(ResponseContentPart::Reasoning {
                                    reasoning: thinking.clone(),
                                });
                            }
                            AnthropicContentBlock::ToolUse {
                                id, name, input, ..
                            } => {
                                if !parts.is_empty() {
                                    items.push(ResponseInputItem::Message {
                                        role: "assistant".to_string(),
                                        content: ResponseInputContent::Parts(std::mem::take(&mut parts)),
                                    });
                                }
                                items.push(ResponseInputItem::FunctionCall {
                                    call_id: id.clone(),
                                    name: name.clone(),
                                    arguments: serde_json::to_string(input)
                                        .unwrap_or_else(|_| "{}".to_string()),
                                });
                            }
                            _ => {}
                        }
                    }
                    if !parts.is_empty() {
                        items.push(ResponseInputItem::Message {
                            role: "assistant".to_string(),
                            content: ResponseInputContent::Parts(parts),
                        });
                    }
                }
            },
            AnthropicRole::System => {
                let text = msg.content.as_plain_text();
                instructions = Some(match instructions.take() {
                    Some(prev) => format!("{}\n\n{}", prev, text),
                    None => text,
                });
            }
            _ => match &msg.content {
                AnthropicContent::Text(t) => {
                    items.push(ResponseInputItem::Message {
                        role: "user".to_string(),
                        content: ResponseInputContent::Text(t.clone()),
                    });
                }
                AnthropicContent::Blocks(blocks) => {
                    let mut parts = Vec::new();
                    for block in blocks {
                        match block {
                            AnthropicContentBlock::Text { text, .. } => {
                                parts.push(ResponseContentPart::Text { text: text.clone() });
                            }
                            AnthropicContentBlock::Image { source, .. } => {
                                let url = if source.r#type == "base64" && !source.data.starts_with("data:") {
                                    format!("data:{};base64,{}", source.media_type, source.data)
                                } else {
                                    source.data.clone()
                                };
                                parts.push(ResponseContentPart::InputImage {
                                    image_url: url,
                                    detail: None,
                                    file_id: None,
                                });
                            }
                            AnthropicContentBlock::Document { source, .. } => {
                                let url = if source.r#type == "base64" && !source.data.starts_with("data:") {
                                    format!("data:{};base64,{}", source.media_type, source.data)
                                } else {
                                    source.data.clone()
                                };
                                parts.push(ResponseContentPart::InputFile {
                                    file_url: Some(url),
                                    file_id: None,
                                    filename: None,
                                });
                            }
                            AnthropicContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } => {
                                if !parts.is_empty() {
                                    items.push(ResponseInputItem::Message {
                                        role: "user".to_string(),
                                        content: ResponseInputContent::Parts(std::mem::take(&mut parts)),
                                    });
                                }
                                let res_text = match content {
                                    ToolResultContent::Text(t) => t.clone(),
                                    ToolResultContent::Blocks(b_list) => b_list
                                        .iter()
                                        .filter_map(|b| match b {
                                            ToolResultBlock::Text { text } => Some(text.as_str()),
                                            _ => None,
                                        })
                                        .collect::<Vec<_>>()
                                        .join(""),
                                };
                                items.push(ResponseInputItem::FunctionResponse {
                                    call_id: tool_use_id.clone(),
                                    output: res_text,
                                });
                            }
                            _ => {}
                        }
                    }
                    if !parts.is_empty() {
                        items.push(ResponseInputItem::Message {
                            role: "user".to_string(),
                            content: ResponseInputContent::Parts(parts),
                        });
                    }
                }
            },
        }
    }

    let input = if items.len() == 1 {
        if let ResponseInputItem::Message { ref content, .. } = items[0] {
            match content {
                ResponseInputContent::Text(t) => ResponseInput::Text(t.clone()),
                _ => ResponseInput::Items(items),
            }
        } else {
            ResponseInput::Items(items)
        }
    } else {
        ResponseInput::Items(items)
    };

    let tools = req.tools.as_ref().map(|t_list| {
        t_list
            .iter()
            .map(|t| ResponseToolDefinition::Function {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: Some(t.input_schema.clone()),
                strict: None,
            })
            .collect()
    });

    let reasoning_effort = req.get_reasoning_effort();
    let reasoning = reasoning_effort.map(|eff| ResponseReasoningConfig {
        effort: Some(eff),
    });

    // Cross-protocol boundary (contract C1/C2): the Responses wire owns
    // `max_output_tokens` / `metadata` / `reasoning`, so a client `extra` entry
    // must not re-inject them on top of the gateway-computed values.
    let extra = sanitize_extra(req.extra.clone(), TargetProtocol::OpenAiResponses);

    Ok(CreateResponseRequest {
        model: req.model.clone(),
        input,
        instructions,
        modalities: None,
        tools,
        tool_choice: None,
        temperature: req.temperature,
        top_p: req.top_p,
        max_output_tokens: Some(req.max_tokens),
        stream: req.stream,
        metadata: None,
        reasoning_effort,
        reasoning,
        extra,
    })
}


/// Convert OpenAI Responses object to Anthropic Messages response.
///
/// Reasoning items become `Thinking` blocks; `FunctionCall` items become `ToolUse`. The message id
/// is normalized to the `msg_` prefix (A-NS-14/A-NS-01) and the cache counters are always present
/// (A-NS-14/A-NS-11).
pub fn responses_to_anthropic_response(resp: &ResponseObject) -> Result<MessageResponse> {
    // Mirror the chat projection: a failed upstream must surface as a
    // Conversion error (failover-eligible) rather than an empty success
    // message with `stop_reason: None`.
    if resp.is_failed() {
        return Err(ProtocolError::Conversion {
            from: "responses",
            to: "anthropic",
            reason: resp.failed_reason(),
        });
    }
    let mut content = Vec::new();
    let mut tool_count = 0u32;

    for item in &resp.output {
        match item {
            ResponseOutputItem::Message { content: parts, .. } => {
                for part in parts {
                    match part {
                        ResponseContentPart::Text { text } => {
                            content.push(AnthropicContentBlock::Text {
                                text: text.clone(),
                                cache_control: None,
                            });
                        }
                        ResponseContentPart::Thought { thought } => {
                            content.push(AnthropicContentBlock::Thinking {
                                thinking: thought.clone(),
                                signature: None,
                            });
                        }
                        ResponseContentPart::Reasoning { reasoning } => {
                            content.push(AnthropicContentBlock::Thinking {
                                thinking: reasoning.clone(),
                                signature: None,
                            });
                        }
                        ResponseContentPart::SummaryText { text }
                        | ResponseContentPart::ReasoningText { text } => {
                            content.push(AnthropicContentBlock::Thinking {
                                thinking: text.clone(),
                                signature: None,
                            });
                        }
                        ResponseContentPart::InputImage { image_url, .. } => {
                            let (media_type, data, src_type) = if image_url.starts_with("data:") {
                                let rest = &image_url["data:".len()..];
                                if let Some((mime, b64)) = rest.split_once(";base64,") {
                                    (mime.to_string(), b64.to_string(), "base64".to_string())
                                } else {
                                    ("image/jpeg".to_string(), image_url.clone(), "url".to_string())
                                }
                            } else {
                                ("image/jpeg".to_string(), image_url.clone(), "url".to_string())
                            };
                            content.push(AnthropicContentBlock::Image {
                                source: AnthropicImageSource {
                                    r#type: src_type,
                                    media_type,
                                    data,
                                },
                                cache_control: None,
                            });
                        }
                        ResponseContentPart::InputFile { file_url, .. } => {
                            if let Some(ref url) = file_url {
                                if url.starts_with("data:") {
                                    let rest = &url["data:".len()..];
                                    if let Some((mime, b64)) = rest.split_once(";base64,") {
                                        content.push(AnthropicContentBlock::Document {
                                            source: AnthropicDocumentSource {
                                                r#type: "base64".to_string(),
                                                media_type: mime.to_string(),
                                                data: b64.to_string(),
                                            },
                                            cache_control: None,
                                        });
                                    }
                                }
                            }
                        }
                        ResponseContentPart::Refusal { .. }
                        | ResponseContentPart::InputAudio { .. }
                        | ResponseContentPart::InputVideo { .. }
                        | ResponseContentPart::Unknown => {}
                    }
                }
            }
            ResponseOutputItem::FunctionCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                tool_count += 1;
                let input_val = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
                content.push(AnthropicContentBlock::ToolUse {
                    id: call_id.clone(),
                    name: name.clone(),
                    input: input_val,
                    cache_control: None,
                });
            }
            ResponseOutputItem::Reasoning { content: parts, summary, .. } => {
                // The conformant reasoning item carries its text both as `reasoning_text`
                // (`content`) and as `summary_text` (`summary`); prefer `content` so the thinking
                // block is not duplicated.
                let thinking = reasoning_text(parts).or_else(|| reasoning_text(summary));
                if let Some(thinking) = thinking {
                    if !thinking.is_empty() {
                        content.push(AnthropicContentBlock::Thinking {
                            thinking,
                            signature: None,
                        });
                    }
                }
            }
            ResponseOutputItem::Unknown => {}
        }
    }

    let stop_reason = if tool_count > 0 {
        Some(AnthropicStopReason::ToolUse)
    } else {
        match resp.status.as_str() {
            "completed" => Some(AnthropicStopReason::EndTurn),
            "incomplete" => Some(AnthropicStopReason::MaxTokens),
            _ => None,
        }
    };

    let usage = match &resp.usage {
        Some(u) => AnthropicUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_creation_input_tokens: Some(u.input_tokens_details.cache_write_tokens),
            cache_read_input_tokens: Some(u.input_tokens_details.cached_tokens),
        },
        None => AnthropicUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: Some(0),
            cache_read_input_tokens: Some(0),
        },
    };

    Ok(MessageResponse {
        id: normalize_anthropic_message_id(&resp.id),
        r#type: "message".to_string(),
        role: "assistant".to_string(),
        content,
        model: resp.model.clone(),
        stop_reason,
        stop_sequence: None,
        usage,
    })
}

/// Joins the textual parts of a reasoning `content`/`summary` array.
fn reasoning_text(parts: &Option<Vec<ResponseContentPart>>) -> Option<String> {
    let parts = parts.as_ref()?;
    let mut acc = String::new();
    for part in parts {
        match part {
            ResponseContentPart::Text { text }
            | ResponseContentPart::SummaryText { text }
            | ResponseContentPart::ReasoningText { text } => acc.push_str(text),
            ResponseContentPart::Thought { thought } => acc.push_str(thought),
            ResponseContentPart::Reasoning { reasoning } => acc.push_str(reasoning),
            _ => {}
        }
    }
    Some(acc)
}

/// Normalizes an upstream/Responses id into the `msg_`-prefixed Anthropic message id
/// (A-NS-01, A-NS-14).
fn normalize_anthropic_message_id(raw: &str) -> String {
    if raw.starts_with("msg_") && raw.len() > "msg_".len() {
        return raw.to_string();
    }
    let tail: String = raw
        .strip_prefix("resp_")
        .unwrap_or(raw)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if tail.is_empty() {
        format!("msg_{:x}", now_unix_secs())
    } else {
        format!("msg_{tail}")
    }
}

/// Convert Anthropic Messages response to OpenAI Responses object.
///
/// `Thinking` blocks become a top-level `reasoning` output item (never an in-message content part);
/// `ToolUse` becomes `FunctionCall` items (R-NS-24).
pub fn anthropic_to_responses_response(resp: &MessageResponse) -> Result<ResponseObject> {
    let mut text_acc = String::new();
    let mut reasoning_acc = String::new();
    let mut tool_items = Vec::new();

    for block in &resp.content {
        match block {
            AnthropicContentBlock::Text { text, .. } => {
                text_acc.push_str(text);
            }
            AnthropicContentBlock::Thinking { thinking, .. } => {
                reasoning_acc.push_str(thinking);
            }
            AnthropicContentBlock::ToolUse {
                id, name, input, ..
            } => {
                tool_items.push(ResponseOutputItem::FunctionCall {
                    id: format!("fc_{}", id),
                    status: "completed".to_string(),
                    call_id: id.clone(),
                    name: name.clone(),
                    arguments: serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string()),
                });
            }
            _ => {}
        }
    }

    let response_id = normalize_response_id(&resp.id);
    let mut output = Vec::new();
    if !reasoning_acc.is_empty() {
        output.push(reasoning_output_item(&response_id, &reasoning_acc));
    }
    if !text_acc.is_empty() {
        output.push(ResponseOutputItem::Message {
            id: normalize_message_item_id(&resp.id),
            status: "completed".to_string(),
            role: "assistant".to_string(),
            content: vec![ResponseContentPart::Text { text: text_acc }],
        });
    }
    output.extend(tool_items);

    let status = match resp.stop_reason {
        Some(AnthropicStopReason::MaxTokens) => "incomplete".to_string(),
        _ => "completed".to_string(),
    };

    let cached_read = resp.usage.cache_read_input_tokens.unwrap_or(0);
    let cached_create = resp.usage.cache_creation_input_tokens.unwrap_or(0);
    let input_tokens = resp.usage.input_tokens + cached_read + cached_create;
    let usage = ResponseUsage::new(
        input_tokens,
        resp.usage.output_tokens,
        cached_read,
        cached_create,
    );

    Ok(ResponseObject {
        id: response_id,
        object: "response".to_string(),
        created_at: now_unix_secs(),
        completed_at: Some(now_unix_secs()),
        status,
        model: resp.model.clone(),
        output,
        usage: Some(usage),
        ..Default::default()
    })
}
