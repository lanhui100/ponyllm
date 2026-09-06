use ponyllm_protocol::openai::responses::*;
use serde_json::json;

#[test]
fn test_responses_input_items_without_type_message() {
    // 许多客户端省略了 "type": "message"，只传 role 与 content
    let req_json = json!({
        "model": "gpt-4o",
        "input": [
            {
                "role": "user",
                "content": "Hello world"
            }
        ]
    });

    let req: CreateResponseRequest = serde_json::from_value(req_json)
        .expect("Should deserialize input items without explicit type='message'");

    match req.input {
        ResponseInput::Items(items) => {
            assert_eq!(items.len(), 1);
            match &items[0] {
                ResponseInputItem::Message { role, content } => {
                    assert_eq!(role, "user");
                    assert_eq!(content.as_plain_text(), "Hello world");
                }
                _ => panic!("Expected Message item"),
            }
        }
        _ => panic!("Expected ResponseInput::Items"),
    }
}

#[test]
fn test_responses_input_content_string_array() {
    // 客户端传纯文本数组 content: ["hello", "world"]
    let req_json = json!({
        "model": "gpt-4o",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": ["hello", "world"]
            }
        ]
    });

    let req: CreateResponseRequest = serde_json::from_value(req_json)
        .expect("Should deserialize content as string array");

    match req.input {
        ResponseInput::Items(items) => {
            assert_eq!(items.len(), 1);
            match &items[0] {
                ResponseInputItem::Message { content, .. } => {
                    assert_eq!(content.as_plain_text(), "hello\nworld");
                }
                _ => panic!("Expected Message item"),
            }
        }
        _ => panic!("Expected ResponseInput::Items"),
    }
}

#[test]
fn test_responses_input_content_multimodal_image() {
    // 包含 input_image 的 content 数组
    let req_json = json!({
        "model": "gpt-4o",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": "What is in this image?"
                    },
                    {
                        "type": "input_image",
                        "image_url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
                    }
                ]
            }
        ]
    });

    let req: CreateResponseRequest = serde_json::from_value(req_json)
        .expect("Should deserialize multimodal image content parts");

    match req.input {
        ResponseInput::Items(items) => {
            assert_eq!(items.len(), 1);
            match &items[0] {
                ResponseInputItem::Message { content, .. } => {
                    match content {
                        ResponseInputContent::Parts(parts) => {
                            assert_eq!(parts.len(), 2);
                            match &parts[1] {
                                ResponseContentPart::InputImage { image_url, .. } => {
                                    assert!(image_url.starts_with("data:image/png;base64,"));
                                }
                                other => panic!("Expected InputImage, got {:?}", other),
                            }
                        }
                        _ => panic!("Expected Parts"),
                    }
                }
                _ => panic!("Expected Message item"),
            }
        }
        _ => panic!("Expected ResponseInput::Items"),
    }
}

#[test]
fn test_responses_input_content_multimodal_audio_and_video() {
    let req_json = json!({
        "model": "gpt-4o",
        "input": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "input_audio",
                        "data": "base64audio...",
                        "format": "wav"
                    },
                    {
                        "type": "input_video",
                        "video_url": "https://example.com/video.mp4"
                    }
                ]
            }
        ]
    });

    let req: CreateResponseRequest = serde_json::from_value(req_json)
        .expect("Should deserialize audio and video content parts");

    match req.input {
        ResponseInput::Items(items) => {
            match &items[0] {
                ResponseInputItem::Message { content, .. } => {
                    match content {
                        ResponseInputContent::Parts(parts) => {
                            assert_eq!(parts.len(), 2);
                            assert!(matches!(&parts[0], ResponseContentPart::InputAudio { .. }));
                            assert!(matches!(&parts[1], ResponseContentPart::InputVideo { .. }));
                        }
                        _ => panic!("Expected Parts"),
                    }
                }
                _ => panic!("Expected Message item"),
            }
        }
        _ => panic!("Expected ResponseInput::Items"),
    }
}
