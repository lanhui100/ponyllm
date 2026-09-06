use crate::error::Result;
use crate::openai::chat::*;
use crate::openai::responses::*;

/// True when a Responses request carries any non-blank text for an upstream
/// that cannot accept image-only input after translation drops images.
pub fn responses_request_has_text(req: &CreateResponseRequest) -> bool {
    if req
        .instructions
        .as_ref()
        .is_some_and(|s| !s.trim().is_empty())
    {
        return true;
    }
    let items: &[ResponseInputItem] = match &req.input {
        ResponseInput::Text(t) => return !t.trim().is_empty(),
        ResponseInput::Items(items) => items,
    };
    items.iter().any(|item| match item {
        ResponseInputItem::Message { content, .. } => content.is_non_empty(),
        ResponseInputItem::FunctionResponse { output, .. } => !output.trim().is_empty(),
        ResponseInputItem::FunctionCall { .. } => false,
    })
}

/// Convert ChatCompletionRequest to CreateResponseRequest
pub fn chat_to_responses_request(req: &ChatCompletionRequest) -> Result<CreateResponseRequest> {
    let mut instructions = None;
    let mut items = Vec::new();

    for msg in &req.messages {
        match msg {
            ChatMessage::System(sys) => {
                instructions = Some(sys.content.as_plain_text());
            }
            ChatMessage::Developer(dev) => {
                instructions = Some(dev.content.as_plain_text());
            }
            ChatMessage::User(user) => {
                let content = match &user.content {
                    MessageContent::Text(t) => ResponseInputContent::Text(t.clone()),
                    MessageContent::Parts(parts) => {
                        let mut resp_parts = Vec::new();
                        for part in parts {
                            match part {
                                ContentPart::Text { text } => {
                                    resp_parts.push(ResponseContentPart::Text { text: text.clone() });
                                }
                                ContentPart::ImageUrl { image_url } => {
                                    resp_parts.push(ResponseContentPart::InputImage {
                                        image_url: image_url.url.clone(),
                                        detail: image_url.detail.clone(),
                                        file_id: None,
                                    });
                                }
                                ContentPart::InputAudio { input_audio } => {
                                    resp_parts.push(ResponseContentPart::InputAudio {
                                        data: input_audio.data.clone(),
                                        format: input_audio.format.clone(),
                                    });
                                }
                                ContentPart::VideoUrl { video_url } => {
                                    resp_parts.push(ResponseContentPart::InputVideo {
                                        video_url: video_url.url.clone(),
                                    });
                                }
                                ContentPart::File { file } => {
                                    resp_parts.push(ResponseContentPart::InputFile {
                                        file_url: file.file_url.clone(),
                                        file_id: file.file_id.clone(),
                                        filename: file.filename.clone(),
                                    });
                                }
                            }
                        }
                        ResponseInputContent::Parts(resp_parts)
                    }
                };
                items.push(ResponseInputItem::Message {
                    role: "user".to_string(),
                    content,
                });
            }
            ChatMessage::Assistant(ast) => {
                let text = ast
                    .content
                    .as_ref()
                    .map(|c| c.as_plain_text())
                    .unwrap_or_default();
                items.push(ResponseInputItem::Message {
                    role: "assistant".to_string(),
                    content: ResponseInputContent::Text(text),
                });
                if let Some(ref tool_calls) = ast.tool_calls {
                    for tc in tool_calls {
                        items.push(ResponseInputItem::FunctionCall {
                            call_id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                        });
                    }
                }
            }
            ChatMessage::Tool(tool) => {
                items.push(ResponseInputItem::FunctionResponse {
                    call_id: tool.tool_call_id.clone(),
                    output: tool.content.as_plain_text(),
                });
            }
            _ => {}
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
                name: t.function.name.clone(),
                description: t.function.description.clone(),
                parameters: t.function.parameters.clone(),
                strict: t.function.strict,
            })
            .collect()
    });

    let reasoning_effort = req.get_reasoning_effort();
    let reasoning = reasoning_effort.map(|eff| ResponseReasoningConfig {
        effort: Some(eff),
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
        max_output_tokens: req.max_completion_tokens.or(req.max_tokens),
        stream: req.stream,
        metadata: None,
        reasoning_effort,
        reasoning,
        extra: req.extra.clone(),
    })
}


/// Convert CreateResponseRequest to ChatCompletionRequest
pub fn responses_to_chat_request(req: &CreateResponseRequest) -> Result<ChatCompletionRequest> {
    let mut messages = Vec::new();

    if let Some(ref inst) = req.instructions {
        messages.push(ChatMessage::System(SystemMessage {
            content: inst.as_str().into(),
            name: None,
        }));
    }

    match &req.input {
        ResponseInput::Text(t) => {
            messages.push(ChatMessage::User(UserMessage {
                content: t.as_str().into(),
                name: None,
            }));
        }
        ResponseInput::Items(items) => {
            let mut pending_calls: Vec<ToolCall> = Vec::new();
            let flush_calls = |messages: &mut Vec<ChatMessage>, pending: &mut Vec<ToolCall>| {
                if pending.is_empty() {
                    return;
                }
                let calls = std::mem::take(pending);
                if let Some(ChatMessage::Assistant(ref mut last_asst)) = messages.last_mut() {
                    match last_asst.tool_calls.as_mut() {
                        Some(existing) => existing.extend(calls),
                        None => last_asst.tool_calls = Some(calls),
                    }
                } else {
                    messages.push(ChatMessage::Assistant(AssistantMessage {
                        content: None,
                        tool_calls: Some(calls),
                        ..Default::default()
                    }));
                }
            };
            for item in items {
                match item {
                    ResponseInputItem::Message { role, content } => {
                        flush_calls(&mut messages, &mut pending_calls);
                        let chat_content = match content {
                            ResponseInputContent::Text(t) => MessageContent::Text(t.clone()),
                            ResponseInputContent::Parts(parts) => {
                                let mut chat_parts = Vec::new();
                                for part in parts {
                                    match part {
                                        ResponseContentPart::Text { text } => {
                                            chat_parts.push(ContentPart::Text { text: text.clone() });
                                        }
                                        ResponseContentPart::InputImage { image_url, detail, .. } => {
                                            chat_parts.push(ContentPart::ImageUrl {
                                                image_url: ImageUrlObject {
                                                    url: image_url.clone(),
                                                    detail: detail.clone(),
                                                },
                                            });
                                        }
                                        ResponseContentPart::InputAudio { data, format } => {
                                            chat_parts.push(ContentPart::InputAudio {
                                                input_audio: InputAudioObject {
                                                    data: data.clone(),
                                                    format: format.clone(),
                                                },
                                            });
                                        }
                                        ResponseContentPart::InputVideo { video_url } => {
                                            chat_parts.push(ContentPart::VideoUrl {
                                                video_url: VideoUrlObject {
                                                    url: video_url.clone(),
                                                },
                                            });
                                        }
                                        ResponseContentPart::InputFile { file_url, file_id, filename } => {
                                            chat_parts.push(ContentPart::File {
                                                file: InputFileObject {
                                                    file_url: file_url.clone(),
                                                    file_id: file_id.clone(),
                                                    filename: filename.clone(),
                                                },
                                            });
                                        }
                                        ResponseContentPart::Thought { thought } => {
                                            chat_parts.push(ContentPart::Text { text: format!("<thought>{}</thought>", thought) });
                                        }
                                        ResponseContentPart::Reasoning { reasoning } => {
                                            chat_parts.push(ContentPart::Text { text: format!("<thought>{}</thought>", reasoning) });
                                        }
                                        ResponseContentPart::Refusal { refusal } => {
                                            chat_parts.push(ContentPart::Text { text: refusal.clone() });
                                        }
                                        ResponseContentPart::Unknown => {}
                                    }
                                }
                                MessageContent::Parts(chat_parts)
                            }
                        };
                        if role == "assistant" {
                            messages.push(ChatMessage::Assistant(AssistantMessage {
                                content: Some(chat_content),
                                ..Default::default()
                            }));
                        } else {
                            messages.push(ChatMessage::User(UserMessage {
                                content: chat_content,
                                name: None,
                            }));
                        }
                    }
                    ResponseInputItem::FunctionCall {
                        call_id,
                        name,
                        arguments,
                    } => {
                        pending_calls.push(ToolCall {
                            id: call_id.clone(),
                            r#type: "function".to_string(),
                            function: FunctionCall {
                                name: name.clone(),
                                arguments: arguments.clone(),
                            },
                        });
                    }
                    ResponseInputItem::FunctionResponse { call_id, output } => {
                        flush_calls(&mut messages, &mut pending_calls);
                        messages.push(ChatMessage::Tool(ToolMessage {
                            content: output.as_str().into(),
                            tool_call_id: call_id.clone(),
                        }));
                    }
                }
            }
            flush_calls(&mut messages, &mut pending_calls);
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
                    strict,
                } => Some(ToolDefinition {
                    r#type: "function".to_string(),
                    function: FunctionDefinition {
                        name: name.clone(),
                        description: description.clone(),
                        parameters: parameters.clone(),
                        strict: *strict,
                    },
                }),
                _ => None,
            })
            .collect()
    });

    let reasoning_effort = req.get_reasoning_effort();

    Ok(ChatCompletionRequest {
        model: req.model.clone(),
        messages,
        temperature: req.temperature,
        top_p: req.top_p,
        n: None,
        stream: req.stream,
        stream_options: None,
        stop: None,
        max_tokens: req.max_output_tokens,
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
        reasoning_effort,
        extra: req.extra.clone(),
    })
}


/// Convert ChatCompletionResponse to ResponseObject
pub fn chat_to_responses_response(resp: &ChatCompletionResponse) -> Result<ResponseObject> {
    let mut text_acc = String::new();
    let mut reasoning_acc = String::new();
    let mut output = Vec::new();
    let mut finish_stop = true;

    if let Some(choice) = resp.choices.first() {
        if let Some(ref reasoning) = choice.message.reasoning_content {
            reasoning_acc.push_str(reasoning);
        }
        if let Some(ref text) = choice.message.content {
            text_acc.push_str(text);
        }
        if let Some(ref tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                output.push(ResponseOutputItem::FunctionCall {
                    id: format!("fc_{}", tc.id),
                    status: "completed".to_string(),
                    call_id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                });
            }
        }
        finish_stop = !matches!(choice.finish_reason, Some(FinishReason::Length));
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

    let usage = resp.usage.as_ref().map(|u| ResponseUsage {
        input_tokens: u.prompt_tokens,
        output_tokens: u.completion_tokens,
        total_tokens: u.total_tokens,
    });

    Ok(ResponseObject {
        id: resp.id.clone(),
        object: "response".to_string(),
        status: if finish_stop {
            "completed".to_string()
        } else {
            "incomplete".to_string()
        },
        model: resp.model.clone(),
        output,
        usage,
        error: None,
    })
}

/// Convert ResponseObject to ChatCompletionResponse
pub fn responses_to_chat_response(resp: &ResponseObject) -> Result<ChatCompletionResponse> {
    let mut text_acc = String::new();
    let mut reasoning_acc = String::new();
    let mut tool_calls = Vec::new();

    for item in &resp.output {
        match item {
            ResponseOutputItem::Message { content, .. } => {
                for part in content {
                    match part {
                        ResponseContentPart::Text { text } => {
                            text_acc.push_str(text);
                        }
                        ResponseContentPart::Thought { thought } => {
                            reasoning_acc.push_str(thought);
                        }
                        ResponseContentPart::Reasoning { reasoning } => {
                            reasoning_acc.push_str(reasoning);
                        }
                        ResponseContentPart::Refusal { .. }
                        | ResponseContentPart::InputImage { .. }
                        | ResponseContentPart::InputAudio { .. }
                        | ResponseContentPart::InputVideo { .. }
                        | ResponseContentPart::InputFile { .. }
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
                tool_calls.push(ToolCall {
                    id: call_id.clone(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: name.clone(),
                        arguments: arguments.clone(),
                    },
                });
            }
            ResponseOutputItem::Reasoning {
                content, summary, ..
            } => {
                if let Some(parts) = content {
                    for part in parts {
                        match part {
                            ResponseContentPart::Text { text } => reasoning_acc.push_str(text),
                            ResponseContentPart::Thought { thought } => {
                                reasoning_acc.push_str(thought)
                            }
                            ResponseContentPart::Reasoning { reasoning } => {
                                reasoning_acc.push_str(reasoning)
                            }
                            _ => {}
                        }
                    }
                }
                if let Some(parts) = summary {
                    for part in parts {
                        match part {
                            ResponseContentPart::Text { text } => reasoning_acc.push_str(text),
                            ResponseContentPart::Thought { thought } => {
                                reasoning_acc.push_str(thought)
                            }
                            ResponseContentPart::Reasoning { reasoning } => {
                                reasoning_acc.push_str(reasoning)
                            }
                            _ => {}
                        }
                    }
                }
            }
            ResponseOutputItem::Unknown => {}
        }
    }

    let content = if text_acc.is_empty() {
        None
    } else {
        Some(text_acc)
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
    let finish_reason = if tool_calls_opt.is_some() {
        Some(FinishReason::ToolCalls)
    } else {
        Some(FinishReason::Stop)
    };

    let usage = resp.usage.as_ref().map(|u| Usage {
        prompt_tokens: u.input_tokens,
        completion_tokens: u.output_tokens,
        total_tokens: u.total_tokens,
        prompt_tokens_details: None,
        completion_tokens_details: None,
    });

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
        usage,
        system_fingerprint: None,
        service_tier: None,
    })
}
