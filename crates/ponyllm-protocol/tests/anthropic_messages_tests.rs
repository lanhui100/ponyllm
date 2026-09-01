use ponyllm_protocol::anthropic::messages::*;
use serde_json::json;

#[test]
fn test_anthropic_message_request_and_response() {
    let req_json = json!({
        "model": "claude-3-5-sonnet-20241022",
        "max_tokens": 1024,
        "system": "You are a senior Rust architect.",
        "messages": [
            {
                "role": "user",
                "content": "How do we model zero-copy streaming in Rust?"
            }
        ],
        "temperature": 0.2,
        "tools": [
            {
                "name": "lookup_docs",
                "description": "Look up Rust standard library docs",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "symbol": {"type": "string"}
                    },
                    "required": ["symbol"]
                }
            }
        ]
    });

    let req: MessageRequest = serde_json::from_value(req_json).unwrap();
    assert_eq!(req.model, "claude-3-5-sonnet-20241022");
    assert_eq!(req.max_tokens, 1024);
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.tools.as_ref().unwrap().len(), 1);

    let resp_json = json!({
        "id": "msg_01XFDUDYCrCwJaSnAZXDgnMp",
        "type": "message",
        "role": "assistant",
        "model": "claude-3-5-sonnet-20241022",
        "content": [
            {
                "type": "thinking",
                "thinking": "The user is asking about zero-copy streaming...",
                "signature": "sig_abc123"
            },
            {
                "type": "text",
                "text": "Zero-copy streaming in Rust is typically achieved using bytes::Bytes and futures::Stream."
            },
            {
                "type": "tool_use",
                "id": "toolu_01A09q90qw90lq917835l1vr",
                "name": "lookup_docs",
                "input": {
                    "symbol": "bytes::Bytes"
                }
            }
        ],
        "stop_reason": "tool_use",
        "stop_sequence": null,
        "usage": {
            "input_tokens": 25,
            "output_tokens": 48,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 10
        }
    });

    let resp: MessageResponse = serde_json::from_value(resp_json).unwrap();
    assert_eq!(resp.id, "msg_01XFDUDYCrCwJaSnAZXDgnMp");
    assert_eq!(resp.stop_reason, Some(AnthropicStopReason::ToolUse));
    assert_eq!(resp.content.len(), 3);
    assert_eq!(resp.usage.cache_read_input_tokens, Some(10));
}

#[test]
fn test_anthropic_stream_events() {
    // 1. MessageStart
    let start_json = json!({
        "type": "message_start",
        "message": {
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-3-5-sonnet-20241022",
            "content": [],
            "stop_reason": null,
            "stop_sequence": null,
            "usage": {
                "input_tokens": 12,
                "output_tokens": 1
            }
        }
    });
    let start_event: MessageStreamEvent = serde_json::from_value(start_json).unwrap();
    if let MessageStreamEvent::MessageStart { message } = start_event {
        assert_eq!(message.id, "msg_123");
    } else {
        panic!("Expected MessageStart");
    }

    // 2. ContentBlockDelta (text_delta)
    let delta_json = json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": {
            "type": "text_delta",
            "text": "Hello world"
        }
    });
    let delta_event: MessageStreamEvent = serde_json::from_value(delta_json).unwrap();
    if let MessageStreamEvent::ContentBlockDelta { index, delta } = delta_event {
        assert_eq!(index, 0);
        if let AnthropicDelta::TextDelta { text } = delta {
            assert_eq!(text, "Hello world");
        } else {
            panic!("Expected TextDelta");
        }
    } else {
        panic!("Expected ContentBlockDelta");
    }

    // 3. ContentBlockDelta (input_json_delta)
    let tool_delta_json = json!({
        "type": "content_block_delta",
        "index": 1,
        "delta": {
            "type": "input_json_delta",
            "partial_json": "{\"sym"
        }
    });
    let tool_delta_event: MessageStreamEvent = serde_json::from_value(tool_delta_json).unwrap();
    if let MessageStreamEvent::ContentBlockDelta { delta, .. } = tool_delta_event {
        if let AnthropicDelta::InputJsonDelta { partial_json } = delta {
            assert_eq!(partial_json, "{\"sym");
        } else {
            panic!("Expected InputJsonDelta");
        }
    } else {
        panic!("Expected ContentBlockDelta");
    }
}
