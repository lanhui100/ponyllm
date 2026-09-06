use ponyllm_protocol::anthropic::messages::*;
use ponyllm_protocol::openai::chat::*;
use ponyllm_protocol::openai::responses::*;
use ponyllm_protocol::translator::*;

#[test]
fn test_chat_to_responses_preserves_image() {
    let req = ChatCompletionRequest {
        model: "muse-spark-1.3".to_string(),
        messages: vec![ChatMessage::User(UserMessage {
            content: MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "Describe this image:".to_string(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrlObject {
                        url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==".to_string(),
                        detail: Some("high".to_string()),
                    },
                },
            ]),
            name: None,
        })],
        ..Default::default()
    };

    let resp_req = chat_to_responses_request(&req).expect("chat_to_responses_request should succeed");

    // input 绝不能被压缩成单一纯文本 Text，必须包含 Image
    match resp_req.input {
        ResponseInput::Items(items) => {
            assert_eq!(items.len(), 1);
            match &items[0] {
                ResponseInputItem::Message { content, .. } => {
                    match content {
                        ResponseInputContent::Parts(parts) => {
                            assert_eq!(parts.len(), 2);
                            assert!(matches!(&parts[0], ResponseContentPart::Text { .. }));
                            match &parts[1] {
                                ResponseContentPart::InputImage { image_url, detail, .. } => {
                                    assert!(image_url.starts_with("data:image/png;base64,"));
                                    assert_eq!(detail.as_deref(), Some("high"));
                                }
                                other => panic!("Expected InputImage, got {:?}", other),
                            }
                        }
                        _ => panic!("Expected Parts with image, got text scalar"),
                    }
                }
                _ => panic!("Expected Message item"),
            }
        }
        ResponseInput::Text(_) => panic!("Input must NOT be compressed to plain text when image is present!"),
    }
}

#[test]
fn test_responses_to_chat_preserves_image() {
    let req = CreateResponseRequest {
        model: "mimo-v2.5".to_string(),
        input: ResponseInput::Items(vec![ResponseInputItem::Message {
            role: "user".to_string(),
            content: ResponseInputContent::Parts(vec![
                ResponseContentPart::Text {
                    text: "Analyze image:".to_string(),
                },
                ResponseContentPart::InputImage {
                    image_url: "https://example.com/test.jpg".to_string(),
                    detail: Some("auto".to_string()),
                    file_id: None,
                },
            ]),
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

    let chat_req = responses_to_chat_request(&req).expect("responses_to_chat_request should succeed");
    assert_eq!(chat_req.messages.len(), 1);
    match &chat_req.messages[0] {
        ChatMessage::User(u) => match &u.content {
            MessageContent::Parts(parts) => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(&parts[0], ContentPart::Text { .. }));
                match &parts[1] {
                    ContentPart::ImageUrl { image_url } => {
                        assert_eq!(image_url.url, "https://example.com/test.jpg");
                        assert_eq!(image_url.detail.as_deref(), Some("auto"));
                    }
                    other => panic!("Expected ImageUrl, got {:?}", other),
                }
            }
            _ => panic!("Expected MessageContent::Parts"),
        },
        _ => panic!("Expected User message"),
    }
}

#[test]
fn test_anthropic_to_responses_preserves_image() {
    let req = MessageRequest {
        model: "claude-3-5".to_string(),
        messages: vec![AnthropicMessage {
            role: AnthropicRole::User,
            content: AnthropicContent::Blocks(vec![
                AnthropicContentBlock::Text {
                    text: "Look at this:".to_string(),
                    cache_control: None,
                },
                AnthropicContentBlock::Image {
                    source: AnthropicImageSource {
                        r#type: "base64".to_string(),
                        media_type: "image/png".to_string(),
                        data: "abc1234==".to_string(),
                    },
                    cache_control: None,
                },
            ]),
        }],
        max_tokens: 1024,
        ..Default::default()
    };

    let resp_req = anthropic_to_responses_request(&req).expect("anthropic_to_responses_request should succeed");
    match resp_req.input {
        ResponseInput::Items(items) => {
            assert_eq!(items.len(), 1);
            match &items[0] {
                ResponseInputItem::Message { content, .. } => match content {
                    ResponseInputContent::Parts(parts) => {
                        assert_eq!(parts.len(), 2);
                        match &parts[1] {
                            ResponseContentPart::InputImage { image_url, .. } => {
                                assert_eq!(image_url, "data:image/png;base64,abc1234==");
                            }
                            other => panic!("Expected InputImage, got {:?}", other),
                        }
                    }
                    _ => panic!("Expected Parts"),
                },
                _ => panic!("Expected Message"),
            }
        }
        _ => panic!("Expected Items"),
    }
}

#[test]
fn test_responses_to_anthropic_preserves_image() {
    let req = CreateResponseRequest {
        model: "claude-3-5".to_string(),
        input: ResponseInput::Items(vec![ResponseInputItem::Message {
            role: "user".to_string(),
            content: ResponseInputContent::Parts(vec![
                ResponseContentPart::InputImage {
                    image_url: "data:image/jpeg;base64,123456==".to_string(),
                    detail: None,
                    file_id: None,
                },
            ]),
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

    let ant_req = responses_to_anthropic_request(&req).expect("responses_to_anthropic_request should succeed");
    assert_eq!(ant_req.messages.len(), 1);
    match &ant_req.messages[0].content {
        AnthropicContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 1);
            match &blocks[0] {
                AnthropicContentBlock::Image { source, .. } => {
                    assert_eq!(source.media_type, "image/jpeg");
                    assert_eq!(source.data, "123456==");
                }
                other => panic!("Expected Anthropic Image block, got {:?}", other),
            }
        }
        _ => panic!("Expected Blocks"),
    }
}
