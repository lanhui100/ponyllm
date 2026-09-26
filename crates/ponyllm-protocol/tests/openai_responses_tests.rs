use ponyllm_protocol::openai::responses::*;
use serde_json::json;

#[test]
fn test_create_response_request_and_object() {
    let req_json = json!({
        "model": "gpt-4o",
        "input": "Write a haiku about Rust programming.",
        "instructions": "Be precise and poetic.",
        "temperature": 0.5,
        "max_output_tokens": 100,
        "stream": false
    });

    let req: CreateResponseRequest = serde_json::from_value(req_json).unwrap();
    assert_eq!(req.model, "gpt-4o");
    assert_eq!(req.instructions.as_deref(), Some("Be precise and poetic."));

    let resp_json = json!({
        "id": "resp_123456",
        "object": "response",
        "status": "completed",
        "model": "gpt-4o",
        "output": [
            {
                "id": "item_001",
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [
                    {
                        "type": "text",
                        "text": "Borrow checker fights,\nFearless concurrency rules,\nFast and memory safe."
                    }
                ]
            }
        ],
        "usage": {
            "total_tokens": 35,
            "input_tokens": 15,
            "output_tokens": 20
        }
    });

    let resp: ResponseObject = serde_json::from_value(resp_json).unwrap();
    assert_eq!(resp.id, "resp_123456");
    assert_eq!(resp.status, "completed");
    assert_eq!(resp.output.len(), 1);
    assert_eq!(resp.usage.as_ref().unwrap().total_tokens, 35);
}

#[test]
fn test_response_stream_event() {
    let event_json = json!({
        "type": "response.text.delta",
        "response_id": "resp_123456",
        "item_id": "item_001",
        "output_index": 0,
        "content_index": 0,
        "delta": "Borrow checker"
    });

    let event: ResponseStreamEvent = serde_json::from_value(event_json).unwrap();
    if let ResponseStreamEvent::TextDelta(d) = event {
        assert_eq!(d.delta, "Borrow checker");
        assert_eq!(d.response_id, "resp_123456");
    } else {
        panic!("Expected TextDelta event");
    }
}

#[test]
fn test_response_input_complex_items_and_reasoning_and_custom() {
    let req_json = json!({
        "model": "gpt-5",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": "Hello"
            },
            {
                "type": "reasoning",
                "content": [
                    {
                        "type": "thought",
                        "thought": "Thinking process step 1"
                    }
                ]
            },
            {
                "type": "item_reference",
                "id": "ref_987654"
            },
            {
                "type": "unknown_future_item",
                "foo": "bar",
                "nested": {
                    "count": 42
                }
            }
        ]
    });

    let req: Result<CreateResponseRequest, _> = serde_json::from_value(req_json);
    assert!(req.is_ok(), "Should successfully deserialize input containing reasoning, item_reference, and arbitrary custom items");
    let req = req.unwrap();
    if let ResponseInput::Items(items) = req.input {
        assert_eq!(items.len(), 4);
        assert!(matches!(items[0], ResponseInputItem::Message { .. }));
        assert!(matches!(items[1], ResponseInputItem::Reasoning { .. }));
        assert!(matches!(items[2], ResponseInputItem::ItemReference { .. }));
        assert!(matches!(items[3], ResponseInputItem::Custom(_)));
    } else {
        panic!("Expected ResponseInput::Items");
    }
}
