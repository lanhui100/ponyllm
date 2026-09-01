use ponyllm_protocol::anthropic::messages::*;
use ponyllm_protocol::openai::chat::*;
use ponyllm_protocol::translator::*;
use serde_json::json;

#[test]
fn test_chat_to_anthropic_request() {
    let chat_req = ChatCompletionRequest {
        model: "claude-3-5-sonnet-20241022".to_string(),
        messages: vec![
            ChatMessage::System(SystemMessage {
                content: "You are a helpful assistant.".into(),
                name: None,
            }),
            ChatMessage::User(UserMessage {
                content: "Calculate 2 + 2".into(),
                name: None,
            }),
            ChatMessage::Assistant(AssistantMessage {
                content: Some("I will calculate this.".into()),
                name: None,
                refusal: None,
                reasoning_content: Some("Simple arithmetic operation.".into()),
                tool_calls: Some(vec![ToolCall {
                    id: "call_abc".to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: "calculator".to_string(),
                        arguments: "{\"expr\":\"2+2\"}".to_string(),
                    },
                }]),
            }),
            ChatMessage::Tool(ToolMessage {
                content: "4".into(),
                tool_call_id: "call_abc".to_string(),
            }),
        ],
        temperature: Some(0.5),
        top_p: None,
        n: None,
        stream: Some(false),
        stream_options: None,
        stop: None,
        max_tokens: Some(1024),
        max_completion_tokens: None,
        presence_penalty: None,
        frequency_penalty: None,
        logit_bias: None,
        user: None,
        response_format: None,
        seed: None,
        tools: Some(vec![ToolDefinition {
            r#type: "function".to_string(),
            function: FunctionDefinition {
                name: "calculator".to_string(),
                description: Some("Evaluates math expression".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "expr": {"type": "string"}
                    }
                })),
                strict: None,
            },
        }]),
        tool_choice: None,
        parallel_tool_calls: None,
        extra: Default::default(),
    };

    let anthropic_req = chat_to_anthropic_request(&chat_req).unwrap();
    assert_eq!(anthropic_req.model, "claude-3-5-sonnet-20241022");
    assert_eq!(anthropic_req.max_tokens, 1024);
    assert_eq!(
        anthropic_req.system,
        Some(AnthropicSystem::Text("You are a helpful assistant.".to_string()))
    );
    assert_eq!(anthropic_req.tools.as_ref().unwrap().len(), 1);
    assert_eq!(anthropic_req.tools.as_ref().unwrap()[0].name, "calculator");

    // Check message sequence (user, assistant with tool_use + thinking, user with tool_result)
    assert_eq!(anthropic_req.messages.len(), 3);
    assert_eq!(anthropic_req.messages[0].role, AnthropicRole::User);
    assert_eq!(anthropic_req.messages[1].role, AnthropicRole::Assistant);
    assert_eq!(anthropic_req.messages[2].role, AnthropicRole::User);

    if let AnthropicContent::Blocks(ref blocks) = anthropic_req.messages[1].content {
        assert_eq!(blocks.len(), 3); // Thinking + Text + ToolUse
        match &blocks[0] {
            AnthropicContentBlock::Thinking { thinking, .. } => {
                assert_eq!(thinking, "Simple arithmetic operation.");
            }
            _ => panic!("Expected thinking block"),
        }
        match &blocks[2] {
            AnthropicContentBlock::ToolUse { id, name, .. } => {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "calculator");
            }
            _ => panic!("Expected tool_use block"),
        }
    } else {
        panic!("Expected blocks in assistant message");
    }
}

#[test]
fn test_anthropic_to_chat_request() {
    let anthropic_req = MessageRequest {
        model: "gpt-4o".to_string(),
        messages: vec![
            AnthropicMessage {
                role: AnthropicRole::User,
                content: AnthropicContent::Text("Hello Claude".to_string()),
            },
            AnthropicMessage {
                role: AnthropicRole::Assistant,
                content: AnthropicContent::Blocks(vec![
                    AnthropicContentBlock::Thinking {
                        thinking: "User greeting.".to_string(),
                        signature: None,
                    },
                    AnthropicContentBlock::Text {
                        text: "Hi there! How can I help?".to_string(),
                        cache_control: None,
                    },
                ]),
            },
        ],
        max_tokens: 2048,
        system: Some(AnthropicSystem::Text("Act as an expert.".to_string())),
        metadata: None,
        stop_sequences: None,
        stream: Some(false),
        temperature: Some(0.7),
        top_p: None,
        top_k: None,
        tools: None,
        tool_choice: None,
        thinking: None,
        extra: Default::default(),
    };

    let chat_req = anthropic_to_chat_request(&anthropic_req).unwrap();
    assert_eq!(chat_req.model, "gpt-4o");
    assert_eq!(chat_req.max_tokens, Some(2048));
    assert_eq!(chat_req.messages.len(), 3);

    // Message 0: System
    if let ChatMessage::System(ref s) = chat_req.messages[0] {
        assert_eq!(s.content.as_plain_text(), "Act as an expert.");
    } else {
        panic!("Expected System message");
    }

    // Message 2: Assistant with reasoning
    if let ChatMessage::Assistant(ref a) = chat_req.messages[2] {
        assert_eq!(a.reasoning_content.as_deref(), Some("User greeting."));
        assert_eq!(a.content.as_ref().unwrap().as_plain_text(), "Hi there! How can I help?");
    } else {
        panic!("Expected Assistant message");
    }
}

#[test]
fn test_anthropic_response_to_chat_response() {
    let anthropic_resp = MessageResponse {
        id: "msg_12345".to_string(),
        r#type: "message".to_string(),
        role: "assistant".to_string(),
        content: vec![
            AnthropicContentBlock::Thinking {
                thinking: "Planning the response...".to_string(),
                signature: None,
            },
            AnthropicContentBlock::Text {
                text: "Here is your answer.".to_string(),
                cache_control: None,
            },
            AnthropicContentBlock::ToolUse {
                id: "call_tool_1".to_string(),
                name: "search".to_string(),
                input: json!({"query": "Rust LLM"}),
                cache_control: None,
            },
        ],
        model: "claude-3-5-sonnet".to_string(),
        stop_reason: Some(AnthropicStopReason::ToolUse),
        stop_sequence: None,
        usage: AnthropicUsage {
            input_tokens: 30,
            output_tokens: 50,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: Some(10),
        },
    };

    let chat_resp = anthropic_to_chat_response(&anthropic_resp).unwrap();
    assert_eq!(chat_resp.id, "msg_12345");
    assert_eq!(chat_resp.model, "claude-3-5-sonnet");
    assert_eq!(chat_resp.choices.len(), 1);
    assert_eq!(chat_resp.choices[0].finish_reason, Some(FinishReason::ToolCalls));

    let msg = &chat_resp.choices[0].message;
    assert_eq!(msg.content.as_deref(), Some("Here is your answer."));
    assert_eq!(msg.reasoning_content.as_deref(), Some("Planning the response..."));
    assert_eq!(msg.tool_calls.as_ref().unwrap().len(), 1);
    assert_eq!(msg.tool_calls.as_ref().unwrap()[0].function.name, "search");
    assert_eq!(
        msg.tool_calls.as_ref().unwrap()[0].function.arguments,
        "{\"query\":\"Rust LLM\"}"
    );

    let usage = chat_resp.usage.unwrap();
    assert_eq!(usage.prompt_tokens, 30);
    assert_eq!(usage.completion_tokens, 50);
    assert_eq!(usage.total_tokens, 80);
    assert_eq!(usage.prompt_tokens_details.unwrap().cached_tokens, Some(10));
}

#[test]
fn test_chat_response_to_anthropic_response() {
    let chat_resp = ChatCompletionResponse {
        id: "chatcmpl-999".to_string(),
        object: "chat.completion".to_string(),
        created: 1710000000,
        model: "gpt-4o".to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: AssistantResponseChoiceMessage {
                role: "assistant".to_string(),
                content: Some("Result text".to_string()),
                reasoning_content: Some("Step by step thought".to_string()),
                refusal: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_001".to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: "do_something".to_string(),
                        arguments: "{\"k\":\"v\"}".to_string(),
                    },
                }]),
            },
            finish_reason: Some(FinishReason::ToolCalls),
            logprobs: None,
        }],
        usage: Some(Usage {
            prompt_tokens: 15,
            completion_tokens: 25,
            total_tokens: 40,
            prompt_tokens_details: None,
            completion_tokens_details: Some(CompletionTokensDetails {
                reasoning_tokens: Some(10),
                ..Default::default()
            }),
        }),
        system_fingerprint: None,
        service_tier: None,
    };

    let anthropic_resp = chat_to_anthropic_response(&chat_resp).unwrap();
    assert_eq!(anthropic_resp.id, "chatcmpl-999");
    assert_eq!(anthropic_resp.stop_reason, Some(AnthropicStopReason::ToolUse));
    assert_eq!(anthropic_resp.content.len(), 3); // Thinking, Text, ToolUse

    match &anthropic_resp.content[0] {
        AnthropicContentBlock::Thinking { thinking, .. } => {
            assert_eq!(thinking, "Step by step thought");
        }
        _ => panic!("Expected thinking block"),
    }
    match &anthropic_resp.content[1] {
        AnthropicContentBlock::Text { text, .. } => {
            assert_eq!(text, "Result text");
        }
        _ => panic!("Expected text block"),
    }
    match &anthropic_resp.content[2] {
        AnthropicContentBlock::ToolUse { id, name, input, .. } => {
            assert_eq!(id, "call_001");
            assert_eq!(name, "do_something");
            assert_eq!(input["k"], "v");
        }
        _ => panic!("Expected tool_use block"),
    }
}

#[test]
fn test_chat_to_responses_and_back() {
    let chat_req = ChatCompletionRequest {
        model: "gpt-4o".to_string(),
        messages: vec![
            ChatMessage::System(SystemMessage {
                content: "Instructions here".into(),
                name: None,
            }),
            ChatMessage::User(UserMessage {
                content: "Hello Responses API".into(),
                name: None,
            }),
        ],
        temperature: Some(0.8),
        top_p: None,
        n: None,
        stream: Some(false),
        stream_options: None,
        stop: None,
        max_tokens: Some(512),
        max_completion_tokens: None,
        presence_penalty: None,
        frequency_penalty: None,
        logit_bias: None,
        user: None,
        response_format: None,
        seed: None,
        tools: None,
        tool_choice: None,
        parallel_tool_calls: None,
        extra: Default::default(),
    };

    let resp_req = chat_to_responses_request(&chat_req).unwrap();
    assert_eq!(resp_req.model, "gpt-4o");
    assert_eq!(resp_req.instructions.as_deref(), Some("Instructions here"));
    assert_eq!(resp_req.max_output_tokens, Some(512));

    let back_chat_req = responses_to_chat_request(&resp_req).unwrap();
    assert_eq!(back_chat_req.model, "gpt-4o");
    assert_eq!(back_chat_req.messages.len(), 2);
}

#[test]
fn test_streaming_anthropic_to_chat_fsm() {
    let mut fsm = AnthropicStreamToChatFsm::new("model-override");

    // 1. message_start
    let start_event = MessageStreamEvent::MessageStart {
        message: MessageResponse {
            id: "msg_stream_1".to_string(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![],
            model: "claude-3-5-sonnet".to_string(),
            stop_reason: None,
            stop_sequence: None,
            usage: AnthropicUsage {
                input_tokens: 20,
                output_tokens: 0,
                ..Default::default()
            },
        },
    };
    let chunks = fsm.process_event(start_event).unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].id, "msg_stream_1");
    assert_eq!(chunks[0].choices[0].delta.role.as_deref(), Some("assistant"));

    // 2. content_block_start (thinking)
    let block_thinking = MessageStreamEvent::ContentBlockStart {
        index: 0,
        content_block: AnthropicContentBlock::Thinking {
            thinking: "".to_string(),
            signature: None,
        },
    };
    let chunks = fsm.process_event(block_thinking).unwrap();
    assert!(chunks.is_empty() || chunks[0].choices[0].delta.reasoning_content.is_some());

    // 3. content_block_delta (thinking_delta)
    let delta_thinking = MessageStreamEvent::ContentBlockDelta {
        index: 0,
        delta: AnthropicDelta::ThinkingDelta {
            thinking: "Let me think...".to_string(),
        },
    };
    let chunks = fsm.process_event(delta_thinking).unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].choices[0].delta.reasoning_content.as_deref(), Some("Let me think..."));

    // 4. content_block_start (tool_use)
    let block_tool = MessageStreamEvent::ContentBlockStart {
        index: 1,
        content_block: AnthropicContentBlock::ToolUse {
            id: "toolu_abc".to_string(),
            name: "fetch_data".to_string(),
            input: json!({}),
            cache_control: None,
        },
    };
    let chunks = fsm.process_event(block_tool).unwrap();
    assert_eq!(chunks.len(), 1);
    let tool_call_delta = &chunks[0].choices[0].delta.tool_calls.as_ref().unwrap()[0];
    assert_eq!(tool_call_delta.id.as_deref(), Some("toolu_abc"));
    assert_eq!(tool_call_delta.function.as_ref().unwrap().name.as_deref(), Some("fetch_data"));

    // 5. content_block_delta (input_json_delta)
    let delta_json = MessageStreamEvent::ContentBlockDelta {
        index: 1,
        delta: AnthropicDelta::InputJsonDelta {
            partial_json: "{\"page\":1}".to_string(),
        },
    };
    let chunks = fsm.process_event(delta_json).unwrap();
    assert_eq!(chunks.len(), 1);
    let tool_call_delta = &chunks[0].choices[0].delta.tool_calls.as_ref().unwrap()[0];
    assert_eq!(tool_call_delta.function.as_ref().unwrap().arguments.as_deref(), Some("{\"page\":1}"));

    // 6. message_delta & message_stop
    let msg_delta = MessageStreamEvent::MessageDelta {
        delta: MessageDeltaBody {
            stop_reason: Some(AnthropicStopReason::ToolUse),
            stop_sequence: None,
        },
        usage: Some(AnthropicDeltaUsage { output_tokens: 35 }),
    };
    let chunks = fsm.process_event(msg_delta).unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].choices[0].finish_reason, Some(FinishReason::ToolCalls));
    assert_eq!(chunks[0].usage.as_ref().unwrap().completion_tokens, 35);
}
