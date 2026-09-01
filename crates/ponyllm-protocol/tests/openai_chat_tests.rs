use ponyllm_protocol::openai::chat::*;
use serde_json::json;

#[test]
fn test_chat_completion_request_basic() {
    let raw_json = json!({
        "model": "gpt-4o",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "Hello, world!"}
        ],
        "temperature": 0.7,
        "max_tokens": 1024,
        "stream": false
    });

    let req: ChatCompletionRequest = serde_json::from_value(raw_json.clone()).unwrap();
    assert_eq!(req.model, "gpt-4o");
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.temperature, Some(0.7));
    assert_eq!(req.max_tokens, Some(1024));
    assert_eq!(req.stream, Some(false));

    let serialized = serde_json::to_value(&req).unwrap();
    assert_eq!(serialized["model"], "gpt-4o");
    assert_eq!(serialized["messages"].as_array().unwrap().len(), 2);
}

#[test]
fn test_chat_completion_request_with_tools_and_reasoning() {
    let raw_json = json!({
        "model": "deepseek-reasoner",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "What's the weather in Tokyo?"}
                ]
            },
            {
                "role": "assistant",
                "content": null,
                "reasoning_content": "The user is asking for weather in Tokyo. I should call get_weather.",
                "tool_calls": [
                    {
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }
                ]
            },
            {
                "role": "tool",
                "tool_call_id": "call_123",
                "content": "{\"temperature\":\"18C\",\"weather\":\"Rainy\"}"
            }
        ],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get current weather for a city",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "city": {"type": "string"}
                        },
                        "required": ["city"]
                    }
                }
            }
        ],
        "tool_choice": "auto",
        "stream": true,
        "stream_options": {
            "include_usage": true
        }
    });

    let req: ChatCompletionRequest = serde_json::from_value(raw_json).unwrap();
    assert_eq!(req.messages.len(), 3);
    assert!(req.stream.unwrap());
    assert!(req.stream_options.as_ref().unwrap().include_usage);
    assert_eq!(req.tools.as_ref().unwrap().len(), 1);

    // Verify assistant reasoning and tool calls
    if let ChatMessage::Assistant(ref ast) = req.messages[1] {
        assert_eq!(ast.reasoning_content.as_deref(), Some("The user is asking for weather in Tokyo. I should call get_weather."));
        assert_eq!(ast.tool_calls.as_ref().unwrap().len(), 1);
        assert_eq!(ast.tool_calls.as_ref().unwrap()[0].function.name, "get_weather");
    } else {
        panic!("Expected Assistant message");
    }
}

#[test]
fn test_chat_completion_response_and_chunk() {
    let resp_json = json!({
        "id": "chatcmpl-abc123xyz",
        "object": "chat.completion",
        "created": 1710000000,
        "model": "gpt-4o-2024-05-13",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Tokyo is currently 18C and rainy.",
                    "reasoning_content": "Summarizing tool output."
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 55,
            "completion_tokens": 12,
            "total_tokens": 67,
            "completion_tokens_details": {
                "reasoning_tokens": 4
            }
        }
    });

    let resp: ChatCompletionResponse = serde_json::from_value(resp_json).unwrap();
    assert_eq!(resp.id, "chatcmpl-abc123xyz");
    assert_eq!(resp.choices[0].finish_reason, Some(FinishReason::Stop));
    assert_eq!(resp.usage.as_ref().unwrap().total_tokens, 67);
    assert_eq!(
        resp.usage.as_ref().unwrap().completion_tokens_details.as_ref().unwrap().reasoning_tokens,
        Some(4)
    );

    // Test streaming chunk
    let chunk_json = json!({
        "id": "chatcmpl-abc123xyz",
        "object": "chat.completion.chunk",
        "created": 1710000000,
        "model": "gpt-4o",
        "choices": [
            {
                "index": 0,
                "delta": {
                    "role": "assistant",
                    "content": "Tokyo",
                    "reasoning_content": "Analyzing..."
                },
                "finish_reason": null
            }
        ]
    });

    let chunk: ChatCompletionChunk = serde_json::from_value(chunk_json).unwrap();
    assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Tokyo"));
    assert_eq!(chunk.choices[0].delta.reasoning_content.as_deref(), Some("Analyzing..."));
    assert_eq!(chunk.choices[0].finish_reason, None);
}
