use crate::anthropic::messages::*;
use crate::error::Result;
use crate::openai::responses::*;
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
                                text_acc.push_str(&t);
                            }
                            ResponseInputContent::Parts(parts) => {
                                for part in parts {
                                    match part {
                                        ResponseContentPart::Text { text } => {
                                            text_acc.push_str(&text);
                                        }
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
                            name: name.clone(),
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
                    name: name.clone(),
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

    Ok(MessageRequest {
        model: req.model.clone(),
        messages,
        max_tokens: req.max_output_tokens.unwrap_or(4096),
        system: req.instructions.clone().map(AnthropicSystem::Text),
        metadata: None,
        stop_sequences: None,
        stream: req.stream,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: None,
        tools,
        tool_choice: None,
        thinking: None,
        extra: req.extra.clone(),
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
                    let mut text_acc = String::new();
                    for block in blocks {
                        match block {
                            AnthropicContentBlock::Text { text, .. } => {
                                text_acc.push_str(text);
                            }
                            AnthropicContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } => {
                                if !text_acc.is_empty() {
                                    items.push(ResponseInputItem::Message {
                                        role: "user".to_string(),
                                        content: ResponseInputContent::Text(std::mem::take(&mut text_acc)),
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
                            AnthropicContentBlock::Image { .. } => {
                                tracing::warn!("dropping image block: Responses input items carry no image part");
                            }
                            _ => {}
                        }
                    }
                    if !text_acc.is_empty() {
                        items.push(ResponseInputItem::Message {
                            role: "user".to_string(),
                            content: ResponseInputContent::Text(text_acc),
                        });
                    }
                }
            },
        }
    }

    let input = if items.len() == 1 {
        if let ResponseInputItem::Message { ref content, .. } = items[0] {
            ResponseInput::Text(content.as_plain_text())
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
        extra: req.extra.clone(),
    })
}

/// Convert OpenAI Responses object to Anthropic Messages response.
/// Reasoning parts become `Thinking` blocks; `FunctionCall` items become `ToolUse`.
pub fn responses_to_anthropic_response(resp: &ResponseObject) -> Result<MessageResponse> {
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
                        ResponseContentPart::Refusal { .. } | ResponseContentPart::Unknown => {}
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
            ResponseOutputItem::Reasoning {
                content: parts,
                summary,
                ..
            } => {
                if let Some(parts) = parts {
                    for part in parts {
                        match part {
                            ResponseContentPart::Text { text } => {
                                content.push(AnthropicContentBlock::Thinking {
                                    thinking: text.clone(),
                                    signature: None,
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
                            ResponseContentPart::Refusal { .. } | ResponseContentPart::Unknown => {}
                        }
                    }
                }
                if let Some(parts) = summary {
                    for part in parts {
                        match part {
                            ResponseContentPart::Text { text } => {
                                content.push(AnthropicContentBlock::Thinking {
                                    thinking: text.clone(),
                                    signature: None,
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
                            ResponseContentPart::Refusal { .. } | ResponseContentPart::Unknown => {}
                        }
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
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
        None => AnthropicUsage::default(),
    };

    Ok(MessageResponse {
        id: resp.id.clone(),
        r#type: "message".to_string(),
        role: "assistant".to_string(),
        content,
        model: resp.model.clone(),
        stop_reason,
        stop_sequence: None,
        usage,
    })
}

/// Convert Anthropic Messages response to OpenAI Responses object.
/// `Thinking` blocks become `Reasoning` parts; `ToolUse` becomes `FunctionCall` items.
pub fn anthropic_to_responses_response(resp: &MessageResponse) -> Result<ResponseObject> {
    let mut text_acc = String::new();
    let mut reasoning_acc = String::new();
    let mut output = Vec::new();

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
                output.push(ResponseOutputItem::FunctionCall {
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

    let mut parts = Vec::new();
    if !reasoning_acc.is_empty() {
        parts.push(ResponseContentPart::Reasoning {
            reasoning: reasoning_acc,
        });
    }
    if !text_acc.is_empty() {
        parts.push(ResponseContentPart::Text { text: text_acc });
    }
    if !parts.is_empty() {
        output.insert(
            0,
            ResponseOutputItem::Message {
                id: format!("msg_{}", resp.id),
                status: "completed".to_string(),
                role: "assistant".to_string(),
                content: parts,
            },
        );
    }

    let status = match resp.stop_reason {
        Some(AnthropicStopReason::MaxTokens) => "incomplete".to_string(),
        _ => "completed".to_string(),
    };

    let cached_read = resp.usage.cache_read_input_tokens.unwrap_or(0);
    let cached_create = resp.usage.cache_creation_input_tokens.unwrap_or(0);
    let input_tokens = resp.usage.input_tokens + cached_read + cached_create;
    let usage = ResponseUsage {
        input_tokens,
        output_tokens: resp.usage.output_tokens,
        total_tokens: input_tokens + resp.usage.output_tokens,
    };

    Ok(ResponseObject {
        id: resp.id.clone(),
        object: "response".to_string(),
        status,
        model: resp.model.clone(),
        output,
        usage: Some(usage),
        error: None,
    })
}
