use ponyllm_protocol::anthropic::messages::*;
use ponyllm_protocol::openai::chat::*;
use ponyllm_protocol::openai::responses::*;
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
    assert_eq!(usage.prompt_tokens, 40); // 30 fresh input + 10 cached read
    assert_eq!(usage.completion_tokens, 50);
    assert_eq!(usage.total_tokens, 90); // 40 prompt + 50 completion
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
fn test_responses_with_reasoning_to_chat_response() {
    let resp_obj = ResponseObject {
        id: "resp_deepseek_123".to_string(),
        object: "response".to_string(),
        status: "completed".to_string(),
        model: "deepseek-reasoner".to_string(),
        output: vec![ResponseOutputItem::Message {
            id: "msg_1".to_string(),
            status: "completed".to_string(),
            role: "assistant".to_string(),
            content: vec![
                ResponseContentPart::Reasoning {
                    reasoning: "DeepSeek step-by-step thinking...".to_string(),
                },
                ResponseContentPart::Text {
                    text: "Final conclusion.".to_string(),
                },
            ],
        }],
        usage: Some(ResponseUsage {
            total_tokens: 50,
            input_tokens: 20,
            output_tokens: 30,
        }),
        error: None,
    };

    let chat_resp = responses_to_chat_response(&resp_obj).unwrap();
    assert_eq!(chat_resp.model, "deepseek-reasoner");
    assert_eq!(
        chat_resp.choices[0].message.reasoning_content.as_deref(),
        Some("DeepSeek step-by-step thinking...")
    );
    assert_eq!(
        chat_resp.choices[0].message.content.as_deref(),
        Some("Final conclusion.")
    );
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

#[test]
fn test_responses_to_anthropic_request_preserves_tools_and_reasoning() {
    let req = CreateResponseRequest {
        model: "m".to_string(),
        input: ResponseInput::Items(vec![
            ResponseInputItem::Message {
                role: "assistant".to_string(),
                content: vec![
                    ResponseContentPart::Reasoning { reasoning: "plan".to_string() },
                    ResponseContentPart::Text { text: "hi".to_string() },
                ],
            },
            ResponseInputItem::FunctionCall {
                call_id: "c1".to_string(),
                name: "get_time".to_string(),
                arguments: "{}".to_string(),
            },
            ResponseInputItem::FunctionResponse {
                call_id: "c1".to_string(),
                output: "noon".to_string(),
            },
        ]),
        instructions: Some("sys".to_string()),
        modalities: None,
        tools: Some(vec![ResponseToolDefinition::Function {
            name: "get_time".to_string(),
            description: None,
            parameters: None,
            strict: None,
        }]),
        tool_choice: None,
        temperature: None,
        top_p: None,
        max_output_tokens: Some(128),
        stream: None,
        metadata: None,
        extra: Default::default(),
    };
    let ant = responses_to_anthropic_request(&req).unwrap();
    assert_eq!(ant.max_tokens, 128);
    assert!(matches!(ant.system, Some(AnthropicSystem::Text(ref s)) if s == "sys"));
    // Consecutive same-role messages are merged: [assistant(text), assistant(tool)] -> one.
    assert_eq!(ant.messages.len(), 2);
    let blocks = match &ant.messages[0].content {
        AnthropicContent::Blocks(b) => b,
        _ => panic!("expected blocks"),
    };
    assert!(matches!(blocks[0], AnthropicContentBlock::Thinking { .. }));
    assert!(blocks.iter().any(|b| matches!(b, AnthropicContentBlock::ToolUse { .. })));
    assert_eq!(ant.tools.unwrap().len(), 1);
    // Roles must strictly alternate for Anthropic upstreams.
    for w in ant.messages.windows(2) {
        assert_ne!(w[0].role, w[1].role, "roles must alternate");
    }
}

#[test]
fn test_concurrent_function_calls_merge_into_single_messages() {
    let items = vec![
        ResponseInputItem::FunctionCall {
            call_id: "c1".to_string(),
            name: "f1".to_string(),
            arguments: "{}".to_string(),
        },
        ResponseInputItem::FunctionCall {
            call_id: "c2".to_string(),
            name: "f2".to_string(),
            arguments: "{}".to_string(),
        },
        ResponseInputItem::FunctionResponse {
            call_id: "c1".to_string(),
            output: "r1".to_string(),
        },
        ResponseInputItem::FunctionResponse {
            call_id: "c2".to_string(),
            output: "r2".to_string(),
        },
    ];
    let req = CreateResponseRequest {
        model: "m".to_string(),
        input: ResponseInput::Items(items),
        instructions: None,
        modalities: None,
        tools: None,
        tool_choice: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        stream: None,
        metadata: None,
        extra: Default::default(),
    };
    let ant = responses_to_anthropic_request(&req).unwrap();
    assert_eq!(ant.messages.len(), 2);
    assert_eq!(ant.messages[0].role, AnthropicRole::Assistant);
    let uses = match &ant.messages[0].content {
        AnthropicContent::Blocks(b) => b,
        _ => panic!("expected blocks"),
    };
    assert_eq!(uses.len(), 2);
    assert_eq!(ant.messages[1].role, AnthropicRole::User);
    let results = match &ant.messages[1].content {
        AnthropicContent::Blocks(b) => b,
        _ => panic!("expected blocks"),
    };
    assert_eq!(results.len(), 2);

    let chat = responses_to_chat_request(&req).unwrap();
    let assistants: Vec<_> = chat
        .messages
        .iter()
        .filter(|m| matches!(m, ChatMessage::Assistant(_)))
        .collect();
    assert_eq!(assistants.len(), 1);
    if let ChatMessage::Assistant(a) = assistants[0] {
        assert_eq!(a.tool_calls.as_ref().unwrap().len(), 2);
    } else {
        panic!("expected assistant");
    }
}

#[test]
fn test_mixed_sequence_never_breaks_anthropic_alternation() {
    let req = CreateResponseRequest {
        model: "m".to_string(),
        input: ResponseInput::Items(vec![
            ResponseInputItem::Message {
                role: "user".to_string(),
                content: vec![ResponseContentPart::Text { text: "q".to_string() }],
            },
            ResponseInputItem::FunctionCall {
                call_id: "c1".to_string(),
                name: "f".to_string(),
                arguments: "{}".to_string(),
            },
            ResponseInputItem::FunctionResponse {
                call_id: "c1".to_string(),
                output: "r".to_string(),
            },
            ResponseInputItem::Message {
                role: "user".to_string(),
                content: vec![ResponseContentPart::Text { text: "follow-up".to_string() }],
            },
        ]),
        instructions: None,
        modalities: None,
        tools: None,
        tool_choice: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        stream: None,
        metadata: None,
        extra: Default::default(),
    };
    let ant = responses_to_anthropic_request(&req).unwrap();
    assert!(!ant.messages.is_empty());
    for w in ant.messages.windows(2) {
        assert_ne!(w[0].role, w[1].role, "roles must alternate across mixed sequences");
    }
    let texts: String = ant
        .messages
        .iter()
        .map(|m| m.content.as_plain_text())
        .collect::<Vec<_>>()
        .join("|");
    assert!(texts.contains('q') && texts.contains("follow-up"));
    let has_result = ant.messages.iter().any(|m| match &m.content {
        AnthropicContent::Blocks(blocks) => blocks.iter().any(|b| matches!(
            b,
            AnthropicContentBlock::ToolResult { content: ToolResultContent::Text(t), .. } if t == "r"
        )),
        _ => false,
    });
    assert!(has_result, "tool result must survive merging");
}

#[test]
fn test_anthropic_to_responses_request_roundtrip() {
    let req = MessageRequest {
        model: "m".to_string(),
        messages: vec![
            AnthropicMessage {
                role: AnthropicRole::User,
                content: AnthropicContent::Text("hello".to_string()),
            },
            AnthropicMessage {
                role: AnthropicRole::Assistant,
                content: AnthropicContent::Blocks(vec![
                    AnthropicContentBlock::Thinking { thinking: "hmm".to_string(), signature: None },
                    AnthropicContentBlock::Text { text: "world".to_string(), cache_control: None },
                ]),
            },
        ],
        max_tokens: 64,
        system: Some(AnthropicSystem::Text("sys".to_string())),
        metadata: None,
        stop_sequences: None,
        stream: None,
        temperature: Some(0.5),
        top_p: None,
        top_k: None,
        tools: None,
        tool_choice: None,
        thinking: None,
        extra: Default::default(),
    };
    let out = anthropic_to_responses_request(&req).unwrap();
    assert_eq!(out.instructions.as_deref(), Some("sys"));
    assert_eq!(out.max_output_tokens, Some(64));
    let items = match &out.input {
        ResponseInput::Items(v) => v,
        _ => panic!("expected items"),
    };
    assert_eq!(items.len(), 2);
    let back = responses_to_anthropic_request(&out).unwrap();
    let text = back.messages[1].content.as_plain_text();
    assert!(text.contains("world"));
}

#[test]
fn test_anthropic_response_to_responses_response() {
    let resp = MessageResponse {
        id: "msg_1".to_string(),
        r#type: "message".to_string(),
        role: "assistant".to_string(),
        content: vec![
            AnthropicContentBlock::Thinking { thinking: "plan".to_string(), signature: None },
            AnthropicContentBlock::Text { text: "done".to_string(), cache_control: None },
            AnthropicContentBlock::ToolUse {
                id: "tu_1".to_string(),
                name: "f".to_string(),
                input: json!({"a": 1}),
                cache_control: None,
            },
        ],
        model: "m".to_string(),
        stop_reason: Some(AnthropicStopReason::ToolUse),
        stop_sequence: None,
        usage: AnthropicUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
    };
    let out = anthropic_to_responses_response(&resp).unwrap();
    assert_eq!(out.status, "completed");
    assert_eq!(out.output.len(), 2);
    let msg = match &out.output[0] {
        ResponseOutputItem::Message { content, .. } => content,
        _ => panic!("expected message first"),
    };
    assert!(matches!(msg[0], ResponseContentPart::Reasoning { .. }));
    assert!(matches!(msg[1], ResponseContentPart::Text { .. }));
    let back = responses_to_anthropic_response(&out).unwrap();
    assert_eq!(back.stop_reason, Some(AnthropicStopReason::ToolUse));
    assert_eq!(back.usage.input_tokens, 10);
    assert_eq!(back.usage.output_tokens, 5);
}

#[test]
fn test_chat_response_to_responses_response() {
    let resp = ChatCompletionResponse {
        id: "chatcmpl-1".to_string(),
        object: "chat.completion".to_string(),
        created: 1,
        model: "m".to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: AssistantResponseChoiceMessage {
                role: "assistant".to_string(),
                content: Some("answer".to_string()),
                reasoning_content: Some("why".to_string()),
                refusal: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: "f".to_string(),
                        arguments: "{\"x\":1}".to_string(),
                    },
                }]),
            },
            finish_reason: Some(FinishReason::ToolCalls),
            logprobs: None,
        }],
        usage: Some(Usage {
            prompt_tokens: 7,
            completion_tokens: 3,
            total_tokens: 10,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        }),
        system_fingerprint: None,
        service_tier: None,
    };
    let out = chat_to_responses_response(&resp).unwrap();
    assert_eq!(out.status, "completed");
    assert_eq!(out.output.len(), 2);
    assert_eq!(out.usage.as_ref().unwrap().total_tokens, 10);
    let back = responses_to_chat_response(&out).unwrap();
    let msg = &back.choices[0].message;
    assert!(msg.content.as_deref().unwrap().contains("answer"));
    assert_eq!(msg.tool_calls.as_ref().unwrap().len(), 1);
}

#[test]
fn test_responses_to_chat_stream_fsm() {
    let mut fsm = ResponsesToChatFsm::new("m");
    let created = ResponseStreamEvent::ResponseCreated {
        response: ResponseObject {
            id: "resp_1".to_string(),
            object: "response".to_string(),
            status: "in_progress".to_string(),
            model: "m".to_string(),
            output: vec![],
            usage: None,
            error: None,
        },
    };
    let chunks = fsm.process_event(created).unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].choices[0].delta.role.as_deref(), Some("assistant"));

    let text = ResponseStreamEvent::OutputTextDelta(ResponseTextDelta {
        response_id: "resp_1".to_string(),
        item_id: "it_0".to_string(),
        output_index: 0,
        content_index: 0,
        delta: "hello".to_string(),
    });
    let chunks = fsm.process_event(text).unwrap();
    assert_eq!(chunks[0].choices[0].delta.content.as_deref(), Some("hello"));

    let done = ResponseStreamEvent::Completed {
        response: ResponseObject {
            id: "resp_1".to_string(),
            object: "response".to_string(),
            status: "completed".to_string(),
            model: "m".to_string(),
            output: vec![],
            usage: Some(ResponseUsage { total_tokens: 9, input_tokens: 6, output_tokens: 3 }),
            error: None,
        },
    };
    let chunks = fsm.process_event(done).unwrap();
    assert_eq!(chunks[0].choices[0].finish_reason, Some(FinishReason::Stop));
    assert_eq!(chunks[0].usage.as_ref().unwrap().total_tokens, 9);
}

#[test]
fn test_chat_to_responses_stream_fsm() {
    let mut fsm = ChatToResponsesFsm::new("m");
    let chunk = ChatCompletionChunk {
        id: "chatcmpl-1".to_string(),
        object: "chat.completion.chunk".to_string(),
        created: 1,
        model: "m".to_string(),
        choices: vec![ChatChunkChoice {
            index: 0,
            delta: ChatChunkDelta {
                role: Some("assistant".to_string()),
                content: Some("hi".to_string()),
                reasoning_content: None,
                refusal: None,
                tool_calls: None,
            },
            finish_reason: None,
            logprobs: None,
        }],
        usage: None,
        system_fingerprint: None,
        service_tier: None,
    };
    let events = fsm.process_chunk(chunk).unwrap();
    assert!(matches!(events[0], ResponseStreamEvent::ResponseCreated { .. }));
    assert!(matches!(events[1], ResponseStreamEvent::OutputTextDelta(_)));

    let fin = ChatCompletionChunk {
        id: "chatcmpl-1".to_string(),
        object: "chat.completion.chunk".to_string(),
        created: 1,
        model: "m".to_string(),
        choices: vec![ChatChunkChoice {
            index: 0,
            delta: ChatChunkDelta::default(),
            finish_reason: Some(FinishReason::Stop),
            logprobs: None,
        }],
        usage: Some(Usage {
            prompt_tokens: 4,
            completion_tokens: 2,
            total_tokens: 6,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        }),
        system_fingerprint: None,
        service_tier: None,
    };
    let events = fsm.process_chunk(fin).unwrap();
    assert!(matches!(events[0], ResponseStreamEvent::Completed { .. }));
    assert!(fsm.finish_if_open().is_none());
}

#[test]
fn test_responses_to_anthropic_stream_fsm() {
    let mut fsm = ResponsesToAnthropicFsm::new("m");
    let created = ResponseStreamEvent::ResponseCreated {
        response: ResponseObject {
            id: "resp_9".to_string(),
            object: "response".to_string(),
            status: "in_progress".to_string(),
            model: "m".to_string(),
            output: vec![],
            usage: None,
            error: None,
        },
    };
    let events = fsm.process_event(created).unwrap();
    assert!(matches!(events[0], MessageStreamEvent::MessageStart { .. }));

    let text = ResponseStreamEvent::TextDelta(ResponseTextDelta {
        response_id: "resp_9".to_string(),
        item_id: "it_0".to_string(),
        output_index: 0,
        content_index: 0,
        delta: "yo".to_string(),
    });
    let events = fsm.process_event(text).unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        MessageStreamEvent::ContentBlockDelta { delta: AnthropicDelta::TextDelta { .. }, .. }
    )));

    let done = ResponseStreamEvent::ResponseDone {
        response: ResponseObject {
            id: "resp_9".to_string(),
            object: "response".to_string(),
            status: "completed".to_string(),
            model: "m".to_string(),
            output: vec![],
            usage: Some(ResponseUsage { total_tokens: 5, input_tokens: 3, output_tokens: 2 }),
            error: None,
        },
    };
    let events = fsm.process_event(done).unwrap();
    assert!(events.iter().any(|e| matches!(e, MessageStreamEvent::MessageStop)));
}

#[test]
fn test_anthropic_to_responses_stream_fsm() {
    let mut fsm = AnthropicToResponsesFsm::new("m");
    let start = MessageStreamEvent::MessageStart {
        message: MessageResponse {
            id: "msg_7".to_string(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![],
            model: "m".to_string(),
            stop_reason: None,
            stop_sequence: None,
            usage: AnthropicUsage::default(),
        },
    };
    let events = fsm.process_event(start).unwrap();
    assert!(matches!(events[0], ResponseStreamEvent::ResponseCreated { .. }));

    let delta = MessageStreamEvent::ContentBlockDelta {
        index: 0,
        delta: AnthropicDelta::TextDelta { text: "hey".to_string() },
    };
    let events = fsm.process_event(delta).unwrap();
    assert!(matches!(events[0], ResponseStreamEvent::OutputTextDelta(_)));

    let stop = MessageStreamEvent::MessageStop;
    let events = fsm.process_event(stop).unwrap();
    assert!(matches!(events[0], ResponseStreamEvent::Completed { .. }));
    assert!(fsm.finish_if_open().is_none());
}

#[test]
fn test_responses_to_chat_request_merges_assistant_text_and_function_call() {
    let req = CreateResponseRequest {
        model: "gpt-4o".to_string(),
        input: ResponseInput::Items(vec![
            ResponseInputItem::Message {
                role: "assistant".to_string(),
                content: vec![ResponseContentPart::Text {
                    text: "I will check the weather.".to_string(),
                }],
            },
            ResponseInputItem::FunctionCall {
                call_id: "call_weather_1".to_string(),
                name: "get_weather".to_string(),
                arguments: "{\"city\":\"Paris\"}".to_string(),
            },
        ]),
        instructions: None,
        modalities: None,
        tools: None,
        tool_choice: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        stream: None,
        metadata: None,
        extra: Default::default(),
    };

    let chat_req = responses_to_chat_request(&req).unwrap();
    assert_eq!(chat_req.messages.len(), 1, "Must merge into a single assistant message rather than consecutive assistant messages");
    match &chat_req.messages[0] {
        ChatMessage::Assistant(a) => {
            assert_eq!(a.content, Some(MessageContent::Text("I will check the weather.".to_string())));
            let calls = a.tool_calls.as_ref().expect("tool_calls must be present");
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].id, "call_weather_1");
            assert_eq!(calls[0].function.name, "get_weather");
        }
        _ => panic!("Expected Assistant message"),
    }
}

#[test]
fn test_anthropic_to_responses_request_preserves_thinking_text_tool_order() {
    let req = MessageRequest {
        model: "claude-3-7-sonnet".to_string(),
        messages: vec![AnthropicMessage {
            role: AnthropicRole::Assistant,
            content: AnthropicContent::Blocks(vec![
                AnthropicContentBlock::Thinking {
                    thinking: "Calculating optimal route".to_string(),
                    signature: None,
                },
                AnthropicContentBlock::Text {
                    text: "I am ready to invoke the routing tool:".to_string(),
                    cache_control: None,
                },
                AnthropicContentBlock::ToolUse {
                    id: "tool_nav_1".to_string(),
                    name: "calculate_route".to_string(),
                    input: json!({"destination": "Mars"}),
                    cache_control: None,
                },
            ]),
        }],
        max_tokens: 1024,
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
        extra: Default::default(),
    };

    let resp_req = anthropic_to_responses_request(&req).unwrap();
    match resp_req.input {
        ResponseInput::Items(items) => {
            assert_eq!(items.len(), 2, "Should emit 1 Message item (thinking+text) followed by 1 FunctionCall item");
            match &items[0] {
                ResponseInputItem::Message { role, content } => {
                    assert_eq!(role, "assistant");
                    assert_eq!(content.len(), 2);
                    assert!(matches!(&content[0], ResponseContentPart::Reasoning { reasoning } if reasoning == "Calculating optimal route"));
                    assert!(matches!(&content[1], ResponseContentPart::Text { text } if text == "I am ready to invoke the routing tool:"));
                }
                _ => panic!("First item must be Message containing reasoning and text"),
            }
            match &items[1] {
                ResponseInputItem::FunctionCall { call_id, name, .. } => {
                    assert_eq!(call_id, "tool_nav_1");
                    assert_eq!(name, "calculate_route");
                }
                _ => panic!("Second item must be FunctionCall"),
            }
        }
        _ => panic!("Expected ResponseInput::Items"),
    }
}

#[test]
fn test_responses_to_anthropic_stream_fsm_empty_delta_and_dynamic_stop_reason() {
    let mut fsm = ResponsesToAnthropicFsm::new("claude-model");
    let _ = fsm.process_event(ResponseStreamEvent::ResponseCreated {
        response: ResponseObject {
            id: "resp_dyn_1".to_string(),
            object: "response".to_string(),
            status: "in_progress".to_string(),
            model: "claude-model".to_string(),
            output: vec![],
            usage: None,
            error: None,
        },
    }).unwrap();

    let empty_events = fsm.process_event(ResponseStreamEvent::TextDelta(ResponseTextDelta {
        response_id: "resp_dyn_1".to_string(),
        item_id: "it_empty".to_string(),
        output_index: 0,
        content_index: 0,
        delta: "".to_string(),
    })).unwrap();
    assert!(empty_events.is_empty(), "Empty text delta must yield zero events");

    let tool_events = fsm.process_event(ResponseStreamEvent::FunctionCallArgumentsDelta(
        ResponseFunctionCallDelta {
            response_id: "resp_dyn_1".to_string(),
            item_id: "call_tool_1".to_string(),
            output_index: 0,
            call_id: "call_tool_1".to_string(),
            delta: "{\"id\":1}".to_string(),
        },
    )).unwrap();
    assert!(tool_events.iter().any(|e| matches!(e, MessageStreamEvent::ContentBlockStart {
        index: 0,
        content_block: AnthropicContentBlock::ToolUse { .. }
    })));

    let finish_events = fsm.finish_if_open().unwrap_or_default();
    assert!(finish_events.iter().any(|e| matches!(
        e,
        MessageStreamEvent::MessageDelta {
            delta: MessageDeltaBody {
                stop_reason: Some(AnthropicStopReason::ToolUse),
                ..
            },
            ..
        }
    )));

    let mut fsm_text = ResponsesToAnthropicFsm::new("claude-model");
    let _ = fsm_text.process_event(ResponseStreamEvent::ResponseCreated {
        response: ResponseObject {
            id: "resp_dyn_2".to_string(),
            object: "response".to_string(),
            status: "in_progress".to_string(),
            model: "claude-model".to_string(),
            output: vec![],
            usage: None,
            error: None,
        },
    }).unwrap();
    let _ = fsm_text.process_event(ResponseStreamEvent::TextDelta(ResponseTextDelta {
        response_id: "resp_dyn_2".to_string(),
        item_id: "it_text".to_string(),
        output_index: 0,
        content_index: 0,
        delta: "Hello".to_string(),
    })).unwrap();
    let text_finish = fsm_text.finish_if_open().unwrap_or_default();
    assert!(text_finish.iter().any(|e| matches!(
        e,
        MessageStreamEvent::MessageDelta {
            delta: MessageDeltaBody {
                stop_reason: Some(AnthropicStopReason::EndTurn),
                ..
            },
            ..
        }
    )));
}
