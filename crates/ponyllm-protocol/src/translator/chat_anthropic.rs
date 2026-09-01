use serde_json::json;
use crate::anthropic::messages::*;
use crate::common::StopCondition;
use crate::error::Result;
use crate::openai::chat::*;

/// Convert OpenAI ChatCompletionRequest to Anthropic MessageRequest
pub fn chat_to_anthropic_request(req: &ChatCompletionRequest) -> Result<MessageRequest> {
    let mut system_prompts = Vec::new();
    let mut anthropic_messages: Vec<AnthropicMessage> = Vec::new();

    for msg in &req.messages {
        match msg {
            ChatMessage::System(sys) => {
                system_prompts.push(sys.content.as_plain_text());
            }
            ChatMessage::Developer(dev) => {
                system_prompts.push(dev.content.as_plain_text());
            }
            ChatMessage::User(user) => {
                let content = match &user.content {
                    MessageContent::Text(t) => AnthropicContent::Text(t.clone()),
                    MessageContent::Parts(parts) => {
                        let mut blocks = Vec::new();
                        for part in parts {
                            match part {
                                ContentPart::Text { text } => {
                                    blocks.push(AnthropicContentBlock::Text {
                                        text: text.clone(),
                                        cache_control: None,
                                    });
                                }
                                ContentPart::ImageUrl { image_url } => {
                                    let (media_type, data, src_type) = if image_url.url.starts_with("data:") {
                                        let rest = &image_url.url["data:".len()..];
                                        if let Some((mime, b64)) = rest.split_once(";base64,") {
                                            (mime.to_string(), b64.to_string(), "base64".to_string())
                                        } else {
                                            ("image/jpeg".to_string(), image_url.url.clone(), "url".to_string())
                                        }
                                    } else {
                                        ("image/jpeg".to_string(), image_url.url.clone(), "url".to_string())
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
                                ContentPart::InputAudio { .. } => {}
                            }
                        }
                        AnthropicContent::Blocks(blocks)
                    }
                };
                anthropic_messages.push(AnthropicMessage {
                    role: AnthropicRole::User,
                    content,
                });
            }
            ChatMessage::Assistant(ast) => {
                let mut blocks = Vec::new();
                if let Some(ref reasoning) = ast.reasoning_content {
                    blocks.push(AnthropicContentBlock::Thinking {
                        thinking: reasoning.clone(),
                        signature: None,
                    });
                }
                if let Some(ref content) = ast.content {
                    blocks.push(AnthropicContentBlock::Text {
                        text: content.as_plain_text(),
                        cache_control: None,
                    });
                }
                if let Some(ref tool_calls) = ast.tool_calls {
                    for tc in tool_calls {
                        let input_val = serde_json::from_str(&tc.function.arguments)
                            .unwrap_or_else(|_| json!({}));
                        blocks.push(AnthropicContentBlock::ToolUse {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            input: input_val,
                            cache_control: None,
                        });
                    }
                }
                anthropic_messages.push(AnthropicMessage {
                    role: AnthropicRole::Assistant,
                    content: AnthropicContent::Blocks(blocks),
                });
            }
            ChatMessage::Tool(tool_msg) => {
                let tool_block = AnthropicContentBlock::ToolResult {
                    tool_use_id: tool_msg.tool_call_id.clone(),
                    content: ToolResultContent::Text(tool_msg.content.as_plain_text()),
                    is_error: None,
                    cache_control: None,
                };
                // If last message was User, append to it; otherwise create new User message
                if let Some(last) = anthropic_messages.last_mut() {
                    if last.role == AnthropicRole::User {
                        match &mut last.content {
                            AnthropicContent::Blocks(blocks) => {
                                blocks.push(tool_block);
                                continue;
                            }
                            AnthropicContent::Text(text) => {
                                let old_text = text.clone();
                                last.content = AnthropicContent::Blocks(vec![
                                    AnthropicContentBlock::Text {
                                        text: old_text,
                                        cache_control: None,
                                    },
                                    tool_block,
                                ]);
                                continue;
                            }
                        }
                    }
                }
                anthropic_messages.push(AnthropicMessage {
                    role: AnthropicRole::User,
                    content: AnthropicContent::Blocks(vec![tool_block]),
                });
            }
            ChatMessage::Function(_) => {}
        }
    }

    let system = if system_prompts.is_empty() {
        None
    } else {
        Some(AnthropicSystem::Text(system_prompts.join("\n\n")))
    };

    let tools = req.tools.as_ref().map(|defs| {
        defs.iter()
            .map(|td| AnthropicTool {
                name: td.function.name.clone(),
                description: td.function.description.clone(),
                input_schema: td.function.parameters.clone().unwrap_or_else(|| json!({"type": "object"})),
                cache_control: None,
            })
            .collect()
    });

    let max_tokens = req
        .max_completion_tokens
        .or(req.max_tokens)
        .unwrap_or(4096);

    let stop_sequences = req.stop.as_ref().map(|s| match s {
        StopCondition::Single(st) => vec![st.clone()],
        StopCondition::Multiple(vec) => vec.clone(),
    });

    Ok(MessageRequest {
        model: req.model.clone(),
        messages: anthropic_messages,
        max_tokens,
        system,
        metadata: None,
        stop_sequences,
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

/// Convert Anthropic MessageRequest to OpenAI ChatCompletionRequest
pub fn anthropic_to_chat_request(req: &MessageRequest) -> Result<ChatCompletionRequest> {
    let mut messages = Vec::new();

    if let Some(ref sys) = req.system {
        let text = match sys {
            AnthropicSystem::Text(t) => t.clone(),
            AnthropicSystem::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    AnthropicSystemBlock::Text { text, .. } => text.as_str(),
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        };
        messages.push(ChatMessage::System(SystemMessage {
            content: text.into(),
            name: None,
        }));
    }

    for msg in &req.messages {
        match msg.role {
            AnthropicRole::User => match &msg.content {
                AnthropicContent::Text(t) => {
                    messages.push(ChatMessage::User(UserMessage {
                        content: t.as_str().into(),
                        name: None,
                    }));
                }
                AnthropicContent::Blocks(blocks) => {
                    let mut user_parts = Vec::new();
                    for block in blocks {
                        match block {
                            AnthropicContentBlock::Text { text, .. } => {
                                user_parts.push(ContentPart::Text { text: text.clone() });
                            }
                            AnthropicContentBlock::Image { source, .. } => {
                                let url = if source.r#type == "base64" && !source.data.starts_with("data:") {
                                    format!("data:{};base64,{}", source.media_type, source.data)
                                } else {
                                    source.data.clone()
                                };
                                user_parts.push(ContentPart::ImageUrl {
                                    image_url: ImageUrlObject {
                                        url,
                                        detail: None,
                                    },
                                });
                            }
                            AnthropicContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } => {
                                if !user_parts.is_empty() {
                                    let parts = std::mem::take(&mut user_parts);
                                    messages.push(ChatMessage::User(UserMessage {
                                        content: MessageContent::Parts(parts),
                                        name: None,
                                    }));
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
                                messages.push(ChatMessage::Tool(ToolMessage {
                                    content: res_text.into(),
                                    tool_call_id: tool_use_id.clone(),
                                }));
                            }
                            _ => {}
                        }
                    }
                    if !user_parts.is_empty() {
                        messages.push(ChatMessage::User(UserMessage {
                            content: MessageContent::Parts(user_parts),
                            name: None,
                        }));
                    }
                }
            },
            AnthropicRole::Assistant => match &msg.content {
                AnthropicContent::Text(t) => {
                    messages.push(ChatMessage::Assistant(AssistantMessage {
                        content: Some(t.as_str().into()),
                        ..Default::default()
                    }));
                }
                AnthropicContent::Blocks(blocks) => {
                    let mut text_acc = String::new();
                    let mut reasoning_acc = String::new();
                    let mut tool_calls = Vec::new();

                    for block in blocks {
                        match block {
                            AnthropicContentBlock::Text { text, .. } => {
                                text_acc.push_str(text);
                            }
                            AnthropicContentBlock::Thinking { thinking, .. } => {
                                reasoning_acc.push_str(thinking);
                            }
                            AnthropicContentBlock::ToolUse { id, name, input, .. } => {
                                tool_calls.push(ToolCall {
                                    id: id.clone(),
                                    r#type: "function".to_string(),
                                    function: FunctionCall {
                                        name: name.clone(),
                                        arguments: serde_json::to_string(input)
                                            .unwrap_or_else(|_| "{}".to_string()),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }

                    let content = if text_acc.is_empty() {
                        None
                    } else {
                        Some(text_acc.into())
                    };

                    let reasoning_content = if reasoning_acc.is_empty() {
                        None
                    } else {
                        Some(reasoning_acc)
                    };

                    let tool_calls_opt = if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    };

                    messages.push(ChatMessage::Assistant(AssistantMessage {
                        content,
                        reasoning_content,
                        tool_calls: tool_calls_opt,
                        ..Default::default()
                    }));
                }
            },
        }
    }

    let tools = req.tools.as_ref().map(|t_list| {
        t_list
            .iter()
            .map(|t| ToolDefinition {
                r#type: "function".to_string(),
                function: FunctionDefinition {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: Some(t.input_schema.clone()),
                    strict: None,
                },
            })
            .collect()
    });

    let stop = req.stop_sequences.as_ref().map(|s| {
        if s.len() == 1 {
            StopCondition::Single(s[0].clone())
        } else {
            StopCondition::Multiple(s.clone())
        }
    });

    Ok(ChatCompletionRequest {
        model: req.model.clone(),
        messages,
        temperature: req.temperature,
        top_p: req.top_p,
        n: None,
        stream: req.stream,
        stream_options: None,
        stop,
        max_tokens: Some(req.max_tokens),
        max_completion_tokens: None,
        presence_penalty: None,
        frequency_penalty: None,
        logit_bias: None,
        user: None,
        response_format: None,
        seed: None,
        tools,
        tool_choice: None,
        parallel_tool_calls: None,
        extra: req.extra.clone(),
    })
}

/// Convert Anthropic MessageResponse to OpenAI ChatCompletionResponse
pub fn anthropic_to_chat_response(resp: &MessageResponse) -> Result<ChatCompletionResponse> {
    let mut text_acc = String::new();
    let mut reasoning_acc = String::new();
    let mut tool_calls = Vec::new();

    for block in &resp.content {
        match block {
            AnthropicContentBlock::Text { text, .. } => {
                text_acc.push_str(text);
            }
            AnthropicContentBlock::Thinking { thinking, .. } => {
                reasoning_acc.push_str(thinking);
            }
            AnthropicContentBlock::ToolUse { id, name, input, .. } => {
                tool_calls.push(ToolCall {
                    id: id.clone(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: name.clone(),
                        arguments: serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string()),
                    },
                });
            }
            _ => {}
        }
    }

    let finish_reason = match resp.stop_reason {
        Some(AnthropicStopReason::EndTurn) => Some(FinishReason::Stop),
        Some(AnthropicStopReason::MaxTokens) => Some(FinishReason::Length),
        Some(AnthropicStopReason::ToolUse) => Some(FinishReason::ToolCalls),
        Some(AnthropicStopReason::StopSequence) => Some(FinishReason::Stop),
        Some(AnthropicStopReason::Other) => Some(FinishReason::Other),
        None => None,
    };

    let content = if text_acc.is_empty() { None } else { Some(text_acc) };
    let reasoning_content = if reasoning_acc.is_empty() { None } else { Some(reasoning_acc) };
    let tool_calls_opt = if tool_calls.is_empty() { None } else { Some(tool_calls) };

    let prompt_cached = resp.usage.cache_read_input_tokens;
    let usage = Usage {
        prompt_tokens: resp.usage.input_tokens,
        completion_tokens: resp.usage.output_tokens,
        total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
        prompt_tokens_details: prompt_cached.map(|c| PromptTokensDetails {
            cached_tokens: Some(c),
            audio_tokens: None,
        }),
        completion_tokens_details: None,
    };

    Ok(ChatCompletionResponse {
        id: resp.id.clone(),
        object: "chat.completion".to_string(),
        created: 0,
        model: resp.model.clone(),
        choices: vec![ChatChoice {
            index: 0,
            message: AssistantResponseChoiceMessage {
                role: "assistant".to_string(),
                content,
                reasoning_content,
                refusal: None,
                tool_calls: tool_calls_opt,
            },
            finish_reason,
            logprobs: None,
        }],
        usage: Some(usage),
        system_fingerprint: None,
        service_tier: None,
    })
}

/// Convert OpenAI ChatCompletionResponse to Anthropic MessageResponse
pub fn chat_to_anthropic_response(resp: &ChatCompletionResponse) -> Result<MessageResponse> {
    let mut content_blocks = Vec::new();
    let mut stop_reason = None;

    if let Some(choice) = resp.choices.first() {
        if let Some(ref reasoning) = choice.message.reasoning_content {
            content_blocks.push(AnthropicContentBlock::Thinking {
                thinking: reasoning.clone(),
                signature: None,
            });
        }
        if let Some(ref text) = choice.message.content {
            content_blocks.push(AnthropicContentBlock::Text {
                text: text.clone(),
                cache_control: None,
            });
        }
        if let Some(ref tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                let input_val = serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({}));
                content_blocks.push(AnthropicContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    input: input_val,
                    cache_control: None,
                });
            }
        }

        stop_reason = choice.finish_reason.as_ref().map(|f| match f {
            FinishReason::Stop => AnthropicStopReason::EndTurn,
            FinishReason::Length => AnthropicStopReason::MaxTokens,
            FinishReason::ToolCalls | FinishReason::FunctionCall => AnthropicStopReason::ToolUse,
            _ => AnthropicStopReason::Other,
        });
    }

    let usage = match &resp.usage {
        Some(u) => AnthropicUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: u.prompt_tokens_details.as_ref().and_then(|d| d.cached_tokens),
        },
        None => AnthropicUsage::default(),
    };

    Ok(MessageResponse {
        id: resp.id.clone(),
        r#type: "message".to_string(),
        role: "assistant".to_string(),
        content: content_blocks,
        model: resp.model.clone(),
        stop_reason,
        stop_sequence: None,
        usage,
    })
}
