use crate::error::Result;
use crate::openai::chat::*;
use crate::openai::responses::*;

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
                items.push(ResponseInputItem::Message {
                    role: "user".to_string(),
                    content: vec![ResponseContentPart::Text {
                        text: user.content.as_plain_text(),
                    }],
                });
            }
            ChatMessage::Assistant(ast) => {
                let text = ast.content.as_ref().map(|c| c.as_plain_text()).unwrap_or_default();
                items.push(ResponseInputItem::Message {
                    role: "assistant".to_string(),
                    content: vec![ResponseContentPart::Text { text }],
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
            if let Some(ResponseContentPart::Text { ref text }) = content.first() {
                ResponseInput::Text(text.clone())
            } else {
                ResponseInput::Items(items)
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
            for item in items {
                match item {
                    ResponseInputItem::Message { role, content } => {
                        let text = content
                            .iter()
                            .filter_map(|c| match c {
                                ResponseContentPart::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("");
                        if role == "assistant" {
                            messages.push(ChatMessage::Assistant(AssistantMessage {
                                content: Some(text.into()),
                                ..Default::default()
                            }));
                        } else {
                            messages.push(ChatMessage::User(UserMessage {
                                content: text.into(),
                                name: None,
                            }));
                        }
                    }
                    ResponseInputItem::FunctionCall {
                        call_id,
                        name,
                        arguments,
                    } => {
                        messages.push(ChatMessage::Assistant(AssistantMessage {
                            content: None,
                            tool_calls: Some(vec![ToolCall {
                                id: call_id.clone(),
                                r#type: "function".to_string(),
                                function: FunctionCall {
                                    name: name.clone(),
                                    arguments: arguments.clone(),
                                },
                            }]),
                            ..Default::default()
                        }));
                    }
                    ResponseInputItem::FunctionResponse { call_id, output } => {
                        messages.push(ChatMessage::Tool(ToolMessage {
                            content: output.as_str().into(),
                            tool_call_id: call_id.clone(),
                        }));
                    }
                }
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
        extra: req.extra.clone(),
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
                        ResponseContentPart::Refusal { .. } => {}
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
        }
    }

    let content = if text_acc.is_empty() { None } else { Some(text_acc) };
    let reasoning_content = if reasoning_acc.is_empty() { None } else { Some(reasoning_acc) };
    let tool_calls_opt = if tool_calls.is_empty() { None } else { Some(tool_calls) };

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
            finish_reason: Some(FinishReason::Stop),
            logprobs: None,
        }],
        usage,
        system_fingerprint: None,
        service_tier: None,
    })
}

