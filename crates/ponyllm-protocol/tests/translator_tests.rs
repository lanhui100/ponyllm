use ponyllm_protocol::anthropic::messages::*;
use ponyllm_protocol::common::ReasoningEffort;
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
        reasoning_effort: None,
        extra: Default::default(),
    };


    let anthropic_req = chat_to_anthropic_request(&chat_req).unwrap();
    assert_eq!(anthropic_req.model, "claude-3-5-sonnet-20241022");
    assert_eq!(anthropic_req.max_tokens, 1024);
    assert_eq!(
        anthropic_req.system,
        Some(AnthropicSystem::Text(
            "You are a helpful assistant.".to_string()
        ))
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
        reasoning_effort: None,
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
        assert_eq!(
            a.content.as_ref().unwrap().as_plain_text(),
            "Hi there! How can I help?"
        );
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
    assert_eq!(
        chat_resp.choices[0].finish_reason,
        Some(FinishReason::ToolCalls)
    );

    let msg = &chat_resp.choices[0].message;
    assert_eq!(msg.content.as_deref(), Some("Here is your answer."));
    assert_eq!(
        msg.reasoning_content.as_deref(),
        Some("Planning the response...")
    );
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
    assert_eq!(
        anthropic_resp.stop_reason,
        Some(AnthropicStopReason::ToolUse)
    );
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
        AnthropicContentBlock::ToolUse {
            id, name, input, ..
        } => {
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
        reasoning_effort: None,
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
    assert_eq!(
        chunks[0].choices[0].delta.role.as_deref(),
        Some("assistant")
    );

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
    assert_eq!(
        chunks[0].choices[0].delta.reasoning_content.as_deref(),
        Some("Let me think...")
    );

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
    assert_eq!(
        tool_call_delta.function.as_ref().unwrap().name.as_deref(),
        Some("fetch_data")
    );

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
    assert_eq!(
        tool_call_delta
            .function
            .as_ref()
            .unwrap()
            .arguments
            .as_deref(),
        Some("{\"page\":1}")
    );

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
    assert_eq!(
        chunks[0].choices[0].finish_reason,
        Some(FinishReason::ToolCalls)
    );
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
                    ResponseContentPart::Reasoning {
                        reasoning: "plan".to_string(),
                    },
                    ResponseContentPart::Text {
                        text: "hi".to_string(),
                    },
                ]
                .into(),
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
        reasoning_effort: None,
        reasoning: None,
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
    assert!(blocks
        .iter()
        .any(|b| matches!(b, AnthropicContentBlock::ToolUse { .. })));
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
        reasoning_effort: None,
        reasoning: None,
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
                content: "q".into(),
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
                content: "follow-up".into(),
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
        reasoning_effort: None,
        reasoning: None,
        extra: Default::default(),
    };

    let ant = responses_to_anthropic_request(&req).unwrap();
    assert!(!ant.messages.is_empty());
    for w in ant.messages.windows(2) {
        assert_ne!(
            w[0].role, w[1].role,
            "roles must alternate across mixed sequences"
        );
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
                    AnthropicContentBlock::Thinking {
                        thinking: "hmm".to_string(),
                        signature: None,
                    },
                    AnthropicContentBlock::Text {
                        text: "world".to_string(),
                        cache_control: None,
                    },
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
        reasoning_effort: None,
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
            AnthropicContentBlock::Thinking {
                thinking: "plan".to_string(),
                signature: None,
            },
            AnthropicContentBlock::Text {
                text: "done".to_string(),
                cache_control: None,
            },
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
    assert_eq!(
        chunks[0].choices[0].delta.role.as_deref(),
        Some("assistant")
    );

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
            usage: Some(ResponseUsage {
                total_tokens: 9,
                input_tokens: 6,
                output_tokens: 3,
            }),
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
    assert!(matches!(
        events[0],
        ResponseStreamEvent::ResponseCreated { .. }
    ));
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
        MessageStreamEvent::ContentBlockDelta {
            delta: AnthropicDelta::TextDelta { .. },
            ..
        }
    )));

    let done = ResponseStreamEvent::ResponseDone {
        response: ResponseObject {
            id: "resp_9".to_string(),
            object: "response".to_string(),
            status: "completed".to_string(),
            model: "m".to_string(),
            output: vec![],
            usage: Some(ResponseUsage {
                total_tokens: 5,
                input_tokens: 3,
                output_tokens: 2,
            }),
            error: None,
        },
    };
    let events = fsm.process_event(done).unwrap();
    assert!(events
        .iter()
        .any(|e| matches!(e, MessageStreamEvent::MessageStop)));
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
    assert!(matches!(
        events[0],
        ResponseStreamEvent::ResponseCreated { .. }
    ));

    let delta = MessageStreamEvent::ContentBlockDelta {
        index: 0,
        delta: AnthropicDelta::TextDelta {
            text: "hey".to_string(),
        },
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
                content: "I will check the weather.".into(),
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
        reasoning_effort: None,
        reasoning: None,
        extra: Default::default(),
    };


    let chat_req = responses_to_chat_request(&req).unwrap();
    assert_eq!(
        chat_req.messages.len(),
        1,
        "Must merge into a single assistant message rather than consecutive assistant messages"
    );
    match &chat_req.messages[0] {
        ChatMessage::Assistant(a) => {
            assert_eq!(
                a.content,
                Some(MessageContent::Text(
                    "I will check the weather.".to_string()
                ))
            );
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
        reasoning_effort: None,
        extra: Default::default(),
    };


    let resp_req = anthropic_to_responses_request(&req).unwrap();
    match resp_req.input {
        ResponseInput::Items(items) => {
            assert_eq!(
                items.len(),
                2,
                "Should emit 1 Message item (thinking+text) followed by 1 FunctionCall item"
            );
            match &items[0] {
                ResponseInputItem::Message { role, content } => {
                    assert_eq!(role, "assistant");
                    match content {
                        ResponseInputContent::Parts(parts) => {
                            assert_eq!(parts.len(), 2);
                            assert!(
                                matches!(&parts[0], ResponseContentPart::Reasoning { reasoning } if reasoning == "Calculating optimal route")
                            );
                            assert!(
                                matches!(&parts[1], ResponseContentPart::Text { text } if text == "I am ready to invoke the routing tool:")
                            );
                        }
                        _ => panic!("Expected ResponseInputContent::Parts"),
                    }
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
    let _ = fsm
        .process_event(ResponseStreamEvent::ResponseCreated {
            response: ResponseObject {
                id: "resp_dyn_1".to_string(),
                object: "response".to_string(),
                status: "in_progress".to_string(),
                model: "claude-model".to_string(),
                output: vec![],
                usage: None,
                error: None,
            },
        })
        .unwrap();

    let empty_events = fsm
        .process_event(ResponseStreamEvent::TextDelta(ResponseTextDelta {
            response_id: "resp_dyn_1".to_string(),
            item_id: "it_empty".to_string(),
            output_index: 0,
            content_index: 0,
            delta: "".to_string(),
        }))
        .unwrap();
    assert!(
        empty_events.is_empty(),
        "Empty text delta must yield zero events"
    );

    let tool_events = fsm
        .process_event(ResponseStreamEvent::FunctionCallArgumentsDelta(
            ResponseFunctionCallDelta {
                response_id: "resp_dyn_1".to_string(),
                item_id: "call_tool_1".to_string(),
                output_index: 0,
                call_id: "call_tool_1".to_string(),
                delta: "{\"id\":1}".to_string(),
            },
        ))
        .unwrap();
    assert!(tool_events.iter().any(|e| matches!(
        e,
        MessageStreamEvent::ContentBlockStart {
            index: 0,
            content_block: AnthropicContentBlock::ToolUse { .. }
        }
    )));

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
    let _ = fsm_text
        .process_event(ResponseStreamEvent::ResponseCreated {
            response: ResponseObject {
                id: "resp_dyn_2".to_string(),
                object: "response".to_string(),
                status: "in_progress".to_string(),
                model: "claude-model".to_string(),
                output: vec![],
                usage: None,
                error: None,
            },
        })
        .unwrap();
    let _ = fsm_text
        .process_event(ResponseStreamEvent::TextDelta(ResponseTextDelta {
            response_id: "resp_dyn_2".to_string(),
            item_id: "it_text".to_string(),
            output_index: 0,
            content_index: 0,
            delta: "Hello".to_string(),
        }))
        .unwrap();
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

#[test]
fn test_refusal_only_input_fails_instead_of_empty_anthropic_message() {
    let req = CreateResponseRequest {
        model: "m".to_string(),
        input: ResponseInput::Items(vec![ResponseInputItem::Message {
            role: "user".to_string(),
            content: vec![ResponseContentPart::Refusal {
                refusal: "no".to_string(),
            }]
            .into(),
        }]),
        instructions: None,
        modalities: None,
        tools: None,
        tool_choice: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        stream: None,
        metadata: None,
        reasoning_effort: None,
        reasoning: None,
        extra: Default::default(),
    };
    let err = responses_to_anthropic_request(&req).unwrap_err();
    assert!(err.to_string().contains("no translatable content"));

    let blank = CreateResponseRequest {
        model: "m".to_string(),
        input: ResponseInput::Text("   ".to_string()),
        instructions: None,
        modalities: None,
        tools: None,
        tool_choice: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        stream: None,
        metadata: None,
        reasoning_effort: None,
        reasoning: None,
        extra: Default::default(),
    };
    assert!(responses_to_anthropic_request(&blank).is_err());
}


#[test]
fn test_responses_reasoning_output_item_and_unknown_deserialization() {
    let raw_json = serde_json::json!({
        "id": "resp_test_reasoning",
        "object": "response",
        "status": "completed",
        "model": "muse-spark-1.3-contributor-free",
        "output": [
            {
                "type": "reasoning",
                "id": "rs_1",
                "content": [
                    { "type": "text", "text": "Deep thinking step 1..." }
                ]
            },
            {
                "type": "message",
                "id": "msg_1",
                "status": "completed",
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "Pong response." }
                ]
            },
            {
                "type": "future_unknown_item_type",
                "id": "un_1",
                "foo": "bar"
            }
        ]
    });

    let resp_obj: ResponseObject =
        serde_json::from_value(raw_json).expect("should deserialize reasoning and unknown items");
    assert_eq!(resp_obj.output.len(), 3);

    let chat = responses_to_chat_response(&resp_obj).expect("should convert to chat completion");
    assert_eq!(
        chat.choices[0].message.content.as_deref(),
        Some("Pong response.")
    );
    assert_eq!(
        chat.choices[0].message.reasoning_content.as_deref(),
        Some("Deep thinking step 1...")
    );
}

#[test]
fn test_responses_output_text_and_encrypted_content_real_upstream() {
    let raw_json = serde_json::json!({
        "id": "resp_6a9b865e03deee5ea22c4054",
        "object": "response",
        "created_at": 1788577374,
        "completed_at": 1788577374,
        "status": "completed",
        "model": "muse-spark-1.3-contributor-free",
        "output": [
            {
                "id": "rs_1",
                "type": "reasoning",
                "status": "completed",
                "encrypted_content": "ciphertext_xyz",
                "summary": []
            },
            {
                "id": "msg_1",
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [
                    {
                        "type": "output_text",
                        "text": "Pong! 👋 How can I help you today?",
                        "annotations": [],
                        "logprobs": []
                    }
                ]
            }
        ]
    });

    let resp_obj: ResponseObject = serde_json::from_value(raw_json)
        .expect("should deserialize output_text and reasoning with encrypted_content");
    assert_eq!(resp_obj.output.len(), 2);

    let chat = responses_to_chat_response(&resp_obj).expect("should convert to chat completion");
    assert_eq!(
        chat.choices[0].message.content.as_deref(),
        Some("Pong! 👋 How can I help you today?")
    );
}

#[test]
fn test_responses_stream_output_text_delta_missing_response_id() {
    let raw_event = serde_json::json!({
        "type": "response.output_text.delta",
        "sequence_number": 6,
        "output_index": 1,
        "content_index": 0,
        "item_id": "msg_01a06f9779c07182b08cde16ffe351bc",
        "delta": "pong",
        "logprobs": []
    });

    let event: ResponseStreamEvent = serde_json::from_value(raw_event)
        .expect("should deserialize output_text.delta even when response_id is omitted");

    let mut fsm = ResponsesToChatFsm::new("test-model");
    let chunks = fsm.process_event(event).expect("fsm should process delta");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].choices[0].delta.content.as_deref(), Some("pong"));
}

#[test]
fn test_chat_to_responses_multi_turn_serializes_cleanly_without_text_type() {
    let chat_req = ChatCompletionRequest {
        model: "muse-spark-1.3-contributor-free".to_string(),
        messages: vec![
            ChatMessage::System(SystemMessage {
                content: "You are an expert software engineer.".into(),
                name: None,
            }),
            ChatMessage::User(UserMessage {
                content: "ping 1".into(),
                name: None,
            }),
            ChatMessage::Assistant(AssistantMessage {
                content: Some("pong 1".into()),
                ..Default::default()
            }),
            ChatMessage::User(UserMessage {
                content: "ping 2".into(),
                name: None,
            }),
        ],
        temperature: None,
        top_p: None,
        n: None,
        stream: Some(false),
        stop: None,
        max_tokens: Some(1024),
        ..Default::default()
    };

    let resp_req = chat_to_responses_request(&chat_req).expect("should convert successfully");
    assert_eq!(
        resp_req.instructions.as_deref(),
        Some("You are an expert software engineer.")
    );

    let serialized = serde_json::to_value(&resp_req).expect("should serialize to json");
    let input = serialized.get("input").expect("input must be present");
    assert!(input.is_array(), "multi-turn input must be an array");

    let items = input.as_array().unwrap();
    assert_eq!(items.len(), 3); // user, assistant, user

    // Verify each message item has a scalar content string and no "type": "text"
    for item in items {
        assert_eq!(item.get("type").and_then(|v| v.as_str()), Some("message"));
        assert!(
            item.get("content").unwrap().is_string(),
            "Message content should serialize as compact scalar string"
        );
    }

    let json_str = serde_json::to_string(&serialized).unwrap();
    assert!(
        !json_str.contains(r#""type":"text""#),
        "Must NOT contain illegal 'type':'text' which is rejected by upstream Responses API"
    );
}

#[test]
fn test_response_input_content_deserializes_scalar_and_parts_aliases() {
    // 1. Scalar string
    let json_scalar = serde_json::json!({
        "type": "message",
        "role": "user",
        "content": "hello world"
    });
    let item: ResponseInputItem = serde_json::from_value(json_scalar).unwrap();
    match item {
        ResponseInputItem::Message { role, content } => {
            assert_eq!(role, "user");
            assert_eq!(content.as_plain_text(), "hello world");
        }
        _ => panic!("Expected Message item"),
    }

    // 2. input_text array
    let json_input_text = serde_json::json!({
        "type": "message",
        "role": "user",
        "content": [
            {"type": "input_text", "text": "hello from input_text"}
        ]
    });
    let item2: ResponseInputItem = serde_json::from_value(json_input_text).unwrap();
    match item2 {
        ResponseInputItem::Message { content, .. } => {
            assert_eq!(content.as_plain_text(), "hello from input_text");
        }
        _ => panic!("Expected Message item"),
    }

    // 3. text / output_text array aliases
    let json_alias = serde_json::json!({
        "type": "message",
        "role": "assistant",
        "content": [
            {"type": "text", "text": "first line"},
            {"type": "output_text", "text": "second line"}
        ]
    });
    let item3: ResponseInputItem = serde_json::from_value(json_alias).unwrap();
    match item3 {
        ResponseInputItem::Message { content, .. } => {
            assert_eq!(content.as_plain_text(), "first line\nsecond line");
        }
        _ => panic!("Expected Message item"),
    }

    // 4. Verify serialization of ResponseContentPart::Text uses "input_text"
    let part = ResponseContentPart::Text {
        text: "out".to_string(),
    };
    let part_json = serde_json::to_value(&part).unwrap();
    assert_eq!(part_json.get("type").and_then(|v| v.as_str()), Some("input_text"));
}

#[test]
fn test_reasoning_effort_parsing_and_serde() {
    use ponyllm_protocol::common::ReasoningEffort;

    // Tolerant string parsing
    assert_eq!(ReasoningEffort::from_str_loose("off"), Some(ReasoningEffort::Off));
    assert_eq!(ReasoningEffort::from_str_loose("none"), Some(ReasoningEffort::Off));
    assert_eq!(ReasoningEffort::from_str_loose("low"), Some(ReasoningEffort::Low));
    assert_eq!(ReasoningEffort::from_str_loose("minimal"), Some(ReasoningEffort::Low));
    assert_eq!(ReasoningEffort::from_str_loose("medium"), Some(ReasoningEffort::Medium));
    assert_eq!(ReasoningEffort::from_str_loose("standard"), Some(ReasoningEffort::Medium));
    assert_eq!(ReasoningEffort::from_str_loose("high"), Some(ReasoningEffort::High));
    assert_eq!(ReasoningEffort::from_str_loose("deep"), Some(ReasoningEffort::High));
    assert_eq!(ReasoningEffort::from_str_loose("max"), Some(ReasoningEffort::High));

    // Serde
    let eff: ReasoningEffort = serde_json::from_str("\"high\"").unwrap();
    assert_eq!(eff, ReasoningEffort::High);
    assert_eq!(serde_json::to_string(&eff).unwrap(), "\"high\"");

    // Ordering: Off < Low < Medium < High
    assert!(ReasoningEffort::Off < ReasoningEffort::Low);
    assert!(ReasoningEffort::Low < ReasoningEffort::Medium);
    assert!(ReasoningEffort::Medium < ReasoningEffort::High);
}

#[test]
fn test_chat_to_anthropic_reasoning_effort_translation() {
    use ponyllm_protocol::common::ReasoningEffort;

    // 1. High effort in chat request
    let chat_req = ChatCompletionRequest {
        model: "claude-opus-5".to_string(),
        messages: vec![ChatMessage::User(UserMessage {
            content: "Think deeply".into(),
            name: None,
        })],
        reasoning_effort: Some(ReasoningEffort::High),
        ..Default::default()
    };
    let ant_req = chat_to_anthropic_request(&chat_req).unwrap();
    assert_eq!(ant_req.reasoning_effort, Some(ReasoningEffort::High));
    let thinking = ant_req.thinking.expect("thinking config should be set");
    assert_eq!(thinking.r#type, "enabled");
    assert_eq!(thinking.effort, Some(ReasoningEffort::High));

    // 2. Off effort in chat request
    let chat_req_off = ChatCompletionRequest {
        model: "claude-opus-5".to_string(),
        messages: vec![ChatMessage::User(UserMessage {
            content: "Quick answer".into(),
            name: None,
        })],
        reasoning_effort: Some(ReasoningEffort::Off),
        ..Default::default()
    };
    let ant_req_off = chat_to_anthropic_request(&chat_req_off).unwrap();
    assert_eq!(ant_req_off.reasoning_effort, Some(ReasoningEffort::Off));
    let thinking_off = ant_req_off.thinking.expect("thinking config should be set");
    assert_eq!(thinking_off.r#type, "disabled");
}

#[test]
fn test_anthropic_to_chat_reasoning_effort_translation() {
    use ponyllm_protocol::common::ReasoningEffort;

    // 1. Anthropic request with thinking effort
    let ant_req = MessageRequest {
        model: "o3-mini".to_string(),
        messages: vec![AnthropicMessage {
            role: AnthropicRole::User,
            content: "Solve math problem".into(),
        }],
        max_tokens: 4096,
        thinking: Some(ThinkingConfig {
            r#type: "enabled".to_string(),
            budget_tokens: None,
            effort: Some(ReasoningEffort::High),
        }),
        ..Default::default()
    };
    let chat_req = anthropic_to_chat_request(&ant_req).unwrap();
    assert_eq!(chat_req.reasoning_effort, Some(ReasoningEffort::High));

    // 2. Anthropic request with budget_tokens fallback calculation
    let ant_req_budget = MessageRequest {
        model: "o3-mini".to_string(),
        messages: vec![AnthropicMessage {
            role: AnthropicRole::User,
            content: "Solve math problem".into(),
        }],
        max_tokens: 4096,
        thinking: Some(ThinkingConfig {
            r#type: "enabled".to_string(),
            budget_tokens: Some(16000),
            effort: None,
        }),
        ..Default::default()
    };
    let chat_req_budget = anthropic_to_chat_request(&ant_req_budget).unwrap();
    assert_eq!(chat_req_budget.reasoning_effort, Some(ReasoningEffort::High));
}

#[test]
fn test_chat_responses_reasoning_effort_bidirectional() {
    use ponyllm_protocol::common::ReasoningEffort;

    // Chat -> Responses
    let chat_req = ChatCompletionRequest {
        model: "fable-5.1".to_string(),
        messages: vec![ChatMessage::User(UserMessage {
            content: "Prove P!=NP".into(),
            name: None,
        })],
        reasoning_effort: Some(ReasoningEffort::High),
        ..Default::default()
    };
    let resp_req = chat_to_responses_request(&chat_req).unwrap();
    assert_eq!(resp_req.reasoning_effort, Some(ReasoningEffort::High));
    assert_eq!(resp_req.reasoning.as_ref().and_then(|r| r.effort), Some(ReasoningEffort::High));

    // Responses -> Chat
    let back_chat = responses_to_chat_request(&resp_req).unwrap();
    assert_eq!(back_chat.reasoning_effort, Some(ReasoningEffort::High));
}

#[test]
fn test_responses_stream_function_call_delta_missing_call_id_routing() {
    use ponyllm_protocol::openai::chat::FinishReason;
    use ponyllm_protocol::openai::responses::ResponseStreamEvent;
    use ponyllm_protocol::translator::ResponsesToChatFsm;

    // 1. Upstream emits OutputItemAdded with id "fc_1" and call_id "call_1"
    let added_json = serde_json::json!({
        "type": "response.output_item.added",
        "output_index": 2,
        "item": {
            "id": "fc_1",
            "type": "function_call",
            "status": "in_progress",
            "name": "list_dir",
            "call_id": "call_1",
            "arguments": ""
        }
    });
    let added_event: ResponseStreamEvent = serde_json::from_value(added_json).unwrap();

    let mut fsm = ResponsesToChatFsm::new("test-model");
    let chunks = fsm.process_event(added_event).unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].choices[0].delta.tool_calls.as_ref().unwrap()[0].id.as_deref(), Some("call_1"));
    assert_eq!(chunks[0].choices[0].delta.tool_calls.as_ref().unwrap()[0].function.as_ref().unwrap().name.as_deref(), Some("list_dir"));

    // 2. Real upstream emits function_call_arguments.delta with item_id but NO call_id
    let arg_delta_json = serde_json::json!({
        "type": "response.function_call_arguments.delta",
        "output_index": 2,
        "item_id": "fc_1",
        "delta": "{\"path\":\".\"}"
    });
    let arg_event: ResponseStreamEvent = serde_json::from_value(arg_delta_json)
        .expect("should deserialize even when call_id is missing");

    let arg_chunks = fsm.process_event(arg_event).unwrap();
    assert_eq!(arg_chunks.len(), 1);
    assert_eq!(
        arg_chunks[0].choices[0].delta.tool_calls.as_ref().unwrap()[0].index,
        0,
        "should map to the same tool index as fc_1"
    );
    assert_eq!(
        arg_chunks[0].choices[0].delta.tool_calls.as_ref().unwrap()[0].function.as_ref().unwrap().arguments.as_deref(),
        Some("{\"path\":\".\"}")
    );

    // 3. Completed event must produce finish_reason: ToolCalls
    let comp_json = serde_json::json!({
        "type": "response.completed",
        "response": {
            "id": "resp_1",
            "object": "response",
            "status": "completed",
            "model": "muse-spark-1.3-contributor-free",
            "output": []
        }
    });
    let comp_event: ResponseStreamEvent = serde_json::from_value(comp_json).unwrap();
    let comp_chunks = fsm.process_event(comp_event).unwrap();
    assert_eq!(comp_chunks.len(), 1);
    assert_eq!(comp_chunks[0].choices[0].finish_reason, Some(FinishReason::ToolCalls));
}

#[test]
fn test_responses_to_anthropic_stream_missing_call_id_routing() {
    use ponyllm_protocol::anthropic::messages::{AnthropicContentBlock, AnthropicDelta, MessageStreamEvent};
    use ponyllm_protocol::openai::responses::ResponseStreamEvent;
    use ponyllm_protocol::translator::ResponsesToAnthropicFsm;

    let added_json = serde_json::json!({
        "type": "response.output_item.added",
        "output_index": 1,
        "item": {
            "id": "fc_upstream_99",
            "type": "function_call",
            "status": "in_progress",
            "name": "read_file",
            "call_id": "call_upstream_99",
            "arguments": ""
        }
    });
    let added_event: ResponseStreamEvent = serde_json::from_value(added_json).unwrap();

    let mut fsm = ResponsesToAnthropicFsm::new("claude-3-5-sonnet");
    let start_events = fsm.process_event(added_event).unwrap();
    assert!(start_events.iter().any(|e| matches!(
        e,
        MessageStreamEvent::ContentBlockStart {
            index: 0,
            content_block: AnthropicContentBlock::ToolUse { id, .. }
        } if id == "call_upstream_99"
    )));

    // Delta carries ONLY item_id, call_id is missing
    let delta_json = serde_json::json!({
        "type": "response.function_call_arguments.delta",
        "output_index": 1,
        "item_id": "fc_upstream_99",
        "delta": "{\"path\":\"README.md\"}"
    });
    let delta_event: ResponseStreamEvent = serde_json::from_value(delta_json).unwrap();
    let delta_events = fsm.process_event(delta_event).unwrap();
    assert_eq!(delta_events.len(), 1);
    match &delta_events[0] {
        MessageStreamEvent::ContentBlockDelta { index, delta } => {
            assert_eq!(*index, 0, "must route to the existing block 0");
            assert!(matches!(delta, AnthropicDelta::InputJsonDelta { partial_json } if partial_json == "{\"path\":\"README.md\"}"));
        }
        _ => panic!("Expected ContentBlockDelta"),
    }
}

#[test]
fn test_chat_to_responses_tool_message_serializes_as_function_call_output() {
    let chat_req = ChatCompletionRequest {
        model: "muse-spark-1.3-contributor-free".to_string(),
        messages: vec![
            ChatMessage::System(SystemMessage {
                content: "You are a coding assistant.".into(),
                name: None,
            }),
            ChatMessage::User(UserMessage {
                content: "Read the file.".into(),
                name: None,
            }),
            ChatMessage::Assistant(AssistantMessage {
                content: Some("Checking file.".into()),
                name: None,
                refusal: None,
                reasoning_content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_123".to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: "read_file".to_string(),
                        arguments: "{\"path\":\"main.rs\"}".to_string(),
                    },
                }]),
            }),
            ChatMessage::Tool(ToolMessage {
                content: "fn main() {}".into(),
                tool_call_id: "call_123".to_string(),
            }),
        ],
        ..Default::default()
    };

    let resp_req = chat_to_responses_request(&chat_req).unwrap();
    let serialized = serde_json::to_value(&resp_req).expect("should serialize to JSON");

    let items = serialized
        .get("input")
        .and_then(|v| v.as_array())
        .expect("input must be an array of items");

    // items: [UserMessage, AssistantMessage, FunctionCall, FunctionCallOutput]
    assert_eq!(items.len(), 4);

    let tool_output_item = &items[3];
    assert_eq!(
        tool_output_item.get("type").and_then(|v| v.as_str()),
        Some("function_call_output"),
        "OpenAI Responses API specification requires 'function_call_output' rather than 'function_response'"
    );
    assert_eq!(
        tool_output_item.get("call_id").and_then(|v| v.as_str()),
        Some("call_123")
    );
    assert_eq!(
        tool_output_item.get("output").and_then(|v| v.as_str()),
        Some("fn main() {}")
    );

    // Roundtrip test: deserialize the standard payload back into CreateResponseRequest
    let roundtrip_req: CreateResponseRequest =
        serde_json::from_value(serialized).expect("should deserialize standard Responses payload");
    if let ResponseInput::Items(ref r_items) = roundtrip_req.input {
        assert!(matches!(
            &r_items[3],
            ResponseInputItem::FunctionResponse { call_id, output }
                if call_id == "call_123" && output == "fn main() {}"
        ));
    } else {
        panic!("expected ResponseInput::Items");
    }

    // Backward-compatibility test: legacy payload with "function_response" must also deserialize
    let legacy_json = serde_json::json!({
        "type": "function_response",
        "call_id": "call_legacy",
        "output": "legacy_output"
    });
    let legacy_item: ResponseInputItem = serde_json::from_value(legacy_json)
        .expect("should deserialize legacy 'function_response' type");
    assert!(matches!(
        legacy_item,
        ResponseInputItem::FunctionResponse { call_id, output }
            if call_id == "call_legacy" && output == "legacy_output"
    ));
}

#[test]
fn test_responses_stream_incomplete_maps_to_finish_reason_length() {
    let incomplete_event_json = serde_json::json!({
        "type": "response.incomplete",
        "response": {
            "id": "resp_trunc",
            "object": "response",
            "status": "incomplete",
            "model": "muse-spark-1.3-contributor-free",
            "output": []
        }
    });
    let event: ResponseStreamEvent = serde_json::from_value(incomplete_event_json)
        .expect("should deserialize response.incomplete event");

    let mut fsm = ResponsesToChatFsm::new("test-model");
    let chunks = fsm.process_event(event).expect("should process incomplete event");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].choices[0].finish_reason, Some(FinishReason::Length));
}

#[test]
fn test_response_object_tolerates_missing_and_null_fields() {
    // Upstream may omit output, object, or usage fields in terminal frame
    let loose_json = serde_json::json!({
        "type": "response.completed",
        "response": {
            "id": "resp_loose",
            "status": "completed",
            "model": "muse-spark-1.3-contributor-free"
        }
    });
    let event: ResponseStreamEvent = serde_json::from_value(loose_json)
        .expect("should deserialize response.completed even when output, object, or usage are missing");
    let mut fsm = ResponsesToChatFsm::new("test-model");
    let chunks = fsm.process_event(event).expect("should process completed");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].choices[0].finish_reason, Some(FinishReason::Stop));
}

#[test]
fn test_antigravity_chunk_to_chat_chunk_function_call() {
    let chunk_val = serde_json::json!({
        "response": {
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "name": "execute_bash",
                            "args": {
                                "command": "cargo test"
                            }
                        }
                    }]
                },
                "finishReason": "STOP"
            }]
        }
    });

    let chunk = antigravity_chunk_to_chat_chunk(&chunk_val, "gemini-3.8-flash-high", "test-resp-1").unwrap();
    assert_eq!(chunk.choices.len(), 1);
    let delta = &chunk.choices[0].delta;
    let tool_calls = delta.tool_calls.as_ref().expect("delta should have tool_calls");
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].function.as_ref().unwrap().name.as_deref(), Some("execute_bash"));
    assert_eq!(tool_calls[0].function.as_ref().unwrap().arguments.as_deref(), Some("{\"command\":\"cargo test\"}"));
    assert_eq!(chunk.choices[0].finish_reason, Some(FinishReason::ToolCalls));
}

#[test]
fn test_antigravity_to_chat_response_function_call() {
    let resp_val = serde_json::json!({
        "response": {
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "name": "view_file",
                            "args": {
                                "path": "Cargo.toml"
                            }
                        }
                    }]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 50,
                "candidatesTokenCount": 25,
                "totalTokenCount": 75
            }
        }
    });

    let chat_resp = antigravity_to_chat_response(&resp_val, "gemini-3.8-flash-high");
    assert_eq!(chat_resp["choices"][0]["finish_reason"], "tool_calls");
    let tool_calls = chat_resp["choices"][0]["message"]["tool_calls"].as_array().expect("tool_calls should be array");
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0]["function"]["name"], "view_file");
    assert_eq!(tool_calls[0]["function"]["arguments"], "{\"path\":\"Cargo.toml\"}");
}

#[test]
fn test_chat_to_antigravity_gemini3_thinking_output_clamped() {
    let req = ChatCompletionRequest {
        model: "gemini-3.8-flash-high".to_string(),
        messages: vec![ChatMessage::User(UserMessage {
            content: "Hello".into(),
            name: None,
        })],
        max_tokens: Some(2048),
        ..Default::default()
    };

    let env = chat_to_antigravity_request(
        &req,
        "gemini-3.8-flash-high",
        "aicode-consumers",
        Some(ReasoningEffort::High),
        "",
    ).unwrap();

    let max_output = env["request"]["generationConfig"]["maxOutputTokens"].as_u64().unwrap();
    assert!(
        max_output >= 16384,
        "Gemini 3 High thinking output tokens must be floored to >= 16384, got {}",
        max_output
    );
}

#[test]
fn test_chat_to_antigravity_tools_and_multi_turn_history() {
    let req = ChatCompletionRequest {
        model: "gemini-3.8-flash-high".to_string(),
        messages: vec![
            ChatMessage::User(UserMessage {
                content: "你好".into(),
                name: None,
            }),
            ChatMessage::User(UserMessage {
                content: "<system-reminder>keep it concise</system-reminder>".into(),
                name: None,
            }),
            ChatMessage::Assistant(AssistantMessage {
                content: None,
                name: None,
                refusal: None,
                reasoning_content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_123".to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: "lookup_stock".to_string(),
                        arguments: "{\"symbol\":\"GOOG\"}".to_string(),
                    },
                }]),
            }),
            ChatMessage::Tool(ToolMessage {
                content: "{\"price\": 180}".into(),
                tool_call_id: "call_123".to_string(),
            }),
        ],
        tools: Some(vec![ToolDefinition {
            r#type: "function".to_string(),
            function: FunctionDefinition {
                name: "lookup_stock".to_string(),
                description: Some("Lookup stock price".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "symbol": {"type": "string"}
                    }
                })),
                strict: None,
            },
        }]),
        ..Default::default()
    };

    let env = chat_to_antigravity_request(
        &req,
        "gemini-3.8-flash-high",
        "aicode-consumers",
        None,
        "",
    ).unwrap();

    let inner = &env["request"];
    // 1. tools should be mapped to functionDeclarations
    let tools = inner["tools"].as_array().expect("tools must be present in request");
    assert_eq!(tools.len(), 1);
    let func_decls = tools[0]["functionDeclarations"].as_array().expect("functionDeclarations present");
    assert_eq!(func_decls[0]["name"], "lookup_stock");

    // 2. consecutive user messages should be merged into a single user turn
    let contents = inner["contents"].as_array().expect("contents array present");
    assert_eq!(contents.len(), 3, "2 merged user turns + 1 model turn + 1 user(tool) turn");
    assert_eq!(contents[0]["role"], "user");
    let user_parts = contents[0]["parts"].as_array().unwrap();
    assert_eq!(user_parts.len(), 2, "Both user texts should be in parts of turn 0");

    // 3. assistant with tool_calls should NOT emit text: "" and should contain functionCall
    assert_eq!(contents[1]["role"], "model");
    let model_parts = contents[1]["parts"].as_array().unwrap();
    assert!(model_parts.iter().all(|p| p.get("text").map(|t| !t.as_str().unwrap().is_empty()).unwrap_or(true)), "No empty text parts");
    assert_eq!(model_parts[0]["functionCall"]["name"], "lookup_stock");
    assert_eq!(model_parts[0]["thoughtSignature"], "skip_thought_signature_validator");

    // 4. tool response should be mapped to functionResponse
    assert_eq!(contents[2]["role"], "user");
    let tool_parts = contents[2]["parts"].as_array().unwrap();
    assert!(tool_parts[0].get("functionResponse").is_some(), "Tool result must be mapped to functionResponse");
}

#[test]
fn test_messages_to_antigravity_tools_and_multi_turn_history() {
    let req = MessageRequest {
        model: "claude-sonnet-4-6".to_string(),
        messages: vec![
            AnthropicMessage {
                role: AnthropicRole::User,
                content: AnthropicContent::Text("What is the weather in Tokyo?".into()),
            },
            AnthropicMessage {
                role: AnthropicRole::Assistant,
                content: AnthropicContent::Blocks(vec![
                    AnthropicContentBlock::ToolUse {
                        id: "toolu_456".to_string(),
                        name: "get_weather".to_string(),
                        input: json!({"city": "Tokyo"}),
                        cache_control: None,
                    },
                ]),
            },
            AnthropicMessage {
                role: AnthropicRole::User,
                content: AnthropicContent::Blocks(vec![
                    AnthropicContentBlock::ToolResult {
                        tool_use_id: "toolu_456".to_string(),
                        content: ToolResultContent::Text("Sunny, 22C".into()),
                        is_error: None,
                        cache_control: None,
                    },
                ]),
            },
        ],
        tools: Some(vec![AnthropicTool {
            name: "get_weather".to_string(),
            description: Some("Get weather for a city".to_string()),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "city": {"type": "string"}
                }
            }),
            cache_control: None,
        }]),
        max_tokens: 2048,
        ..Default::default()
    };

    let env = messages_to_antigravity_request(
        &req,
        "claude-sonnet-4-6",
        "aicode-consumers",
        None,
        "",
    ).unwrap();

    let inner = &env["request"];
    let tools = inner["tools"].as_array().expect("tools must be present");
    assert_eq!(tools.len(), 1);
    let decls = tools[0]["functionDeclarations"].as_array().unwrap();
    assert_eq!(decls[0]["name"], "get_weather");

    let contents = inner["contents"].as_array().expect("contents array present");
    assert_eq!(contents.len(), 3);
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(contents[1]["parts"][0]["functionCall"]["name"], "get_weather");
    assert_eq!(contents[1]["parts"][0]["thoughtSignature"], "skip_thought_signature_validator");
    assert_eq!(contents[2]["role"], "user");
    assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "get_weather");
}

#[test]
fn test_antigravity_tool_schema_sanitization_removes_unsupported_fields() {
    use ponyllm_protocol::translator::antigravity::sanitize_gemini_schema;

    // Simulate complex JSON Schema sent by Anthropic / OpenAI agent frameworks (e.g. Claude Code, Goose, LangChain)
    let dirty_schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "execute_command",
        "description": "Execute a shell command",
        "type": "object",
        "additionalProperties": false,
        "propertyNames": { "pattern": "^[a-z_]+$" },
        "$defs": {
            "CustomType": { "type": "string" }
        },
        "required": ["command"],
        "properties": {
            "command": {
                "type": "string",
                "description": "Command to run"
            },
            "timeout": {
                "type": "integer",
                "exclusiveMinimum": 0,
                "minimum": 1
            },
            "mode": {
                "const": "safe",
                "description": "Execution mode"
            },
            "retries": {
                "type": ["integer", "null"],
                "default": 3
            },
            "nested_options": {
                "type": "object",
                "propertyNames": { "maxLength": 10 },
                "properties": {
                    "env": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "propertyNames": { "pattern": ".*" },
                            "properties": {
                                "action": {
                                    "anyOf": [
                                        { "const": "set" },
                                        { "const": "unset" }
                                    ]
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let cleaned = sanitize_gemini_schema(&dirty_schema);

    // Root level check
    assert!(cleaned.get("$schema").is_none(), "$schema must be stripped");
    assert!(cleaned.get("additionalProperties").is_none(), "additionalProperties must be stripped");
    assert!(cleaned.get("propertyNames").is_none(), "propertyNames must be stripped");
    assert!(cleaned.get("$defs").is_none(), "$defs must be stripped");
    assert_eq!(cleaned["type"], "object");
    assert_eq!(cleaned["title"], "execute_command");
    assert_eq!(cleaned["description"], "Execute a shell command");

    // Properties check
    let props = &cleaned["properties"];
    assert_eq!(props["command"]["type"], "string");

    // timeout: exclusiveMinimum removed, minimum preserved
    assert!(props["timeout"].get("exclusiveMinimum").is_none(), "exclusiveMinimum must be stripped");
    assert_eq!(props["timeout"]["minimum"], 1);

    // mode: const converted to enum
    assert!(props["mode"].get("const").is_none(), "const must be removed");
    assert_eq!(props["mode"]["enum"], json!(["safe"]), "const converted to enum array");

    // retries: type ["integer", "null"] flattened to type integer + nullable true
    assert_eq!(props["retries"]["type"], "integer");
    assert_eq!(props["retries"]["nullable"], true);
    assert_eq!(props["retries"]["default"], 3);

    // nested checks
    let nested = &props["nested_options"];
    assert!(nested.get("propertyNames").is_none());
    let item_props = &nested["properties"]["env"]["items"];
    assert!(item_props.get("propertyNames").is_none());

    let action_any_of = item_props["properties"]["action"]["anyOf"].as_array().unwrap();
    assert_eq!(action_any_of[0]["enum"], json!(["set"]));
    assert_eq!(action_any_of[1]["enum"], json!(["unset"]));
    assert!(action_any_of[0].get("const").is_none());
    assert!(action_any_of[1].get("const").is_none());
}

#[test]
fn test_messages_to_antigravity_sanitizes_tools_end_to_end() {
    let req = MessageRequest {
        model: "gemini-3.8-flash[1m]".to_string(),
        messages: vec![
            AnthropicMessage {
                role: AnthropicRole::User,
                content: AnthropicContent::Text("run tool".into()),
            },
        ],
        tools: Some(vec![AnthropicTool {
            name: "test_tool".to_string(),
            description: Some("Tool with dirty schema".to_string()),
            input_schema: json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "propertyNames": { "pattern": "^[a-z]+$" },
                "properties": {
                    "foo": {
                        "const": "bar",
                        "propertyNames": {}
                    }
                }
            }),
            cache_control: None,
        }]),
        max_tokens: 1024,
        ..Default::default()
    };

    let env = messages_to_antigravity_request(
        &req,
        "gemini-3.8-flash",
        "aicode-consumers",
        None,
        "",
    ).unwrap();

    let tools = env["request"]["tools"].as_array().unwrap();
    let decls = tools[0]["functionDeclarations"].as_array().unwrap();
    let params = &decls[0]["parameters"];

    assert!(params.get("$schema").is_none());
    assert!(params.get("propertyNames").is_none());
    assert_eq!(params["properties"]["foo"]["enum"], json!(["bar"]));
    assert!(params["properties"]["foo"].get("const").is_none());
    assert!(params["properties"]["foo"].get("propertyNames").is_none());
}


