use axum::{routing::post, Json, Router};
use serde_json::json;
use ponyllm::prelude::*;

#[tokio::test]
async fn test_embedded_sdk_in_memory_call() {
    // 1. Mock upstream server
    let mock_upstream = Router::new().route(
        "/v1/chat/completions",
        post(|Json(req): Json<serde_json::Value>| async move {
            let user_text = req["messages"][0]["content"].as_str().unwrap_or_default();
            axum::Json(json!({
                "id": "chatcmpl-embedded-123",
                "object": "chat.completion",
                "created": 1710000000,
                "model": "mock-model",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": format!("Embedded Response: {}", user_text)
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 5,
                    "completion_tokens": 10,
                    "total_tokens": 15
                }
            }))
        }),
    );

    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock_upstream).await.unwrap();
    });

    // 2. Build embedded PonyGateway SDK instance
    let gateway = PonyGateway::builder()
        .add_provider(
            "openai",
            format!("http://{}", upstream_addr),
            "mock-model",
            RoutingStrategy::RoundRobin,
        )
        .add_key("openai", "key-1", "sk-mock-key-1", 1, 10)
        .build();

    // 3. Invoke directly via Rust API (no gateway HTTP port needed!)
    let req = ChatCompletionRequest {
        model: "mock-model".to_string(),
        messages: vec![ChatMessage::User(UserMessage {
            content: "Hello from embedded SDK".into(),
            name: None,
        })],
        ..Default::default()
    };

    let resp = gateway.chat_completion(&req).await.unwrap();
    assert_eq!(
        resp.choices[0].message.content.as_deref(),
        Some("Embedded Response: Hello from embedded SDK")
    );

    // 4. Anthropic Messages API in-memory translation call
    let ant_req = MessageRequest {
        model: "mock-model".to_string(),
        messages: vec![AnthropicMessage {
            role: AnthropicRole::User,
            content: "Hello Anthropic In-Memory".into(),
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

    let ant_resp = gateway.create_message(&ant_req).await.unwrap();
    assert_eq!(ant_resp.role, "assistant");
    assert_eq!(
        ant_resp.content[0],
        AnthropicContentBlock::Text {
            text: "Embedded Response: Hello Anthropic In-Memory".to_string(),
            cache_control: None,
        }
    );
}

#[tokio::test]
async fn test_embedded_sdk_anthropic_upstream_direct() {
    let mock_anthropic = Router::new().route(
        "/anthropic/v1/messages",
        post(|Json(req): Json<serde_json::Value>| async move {
            let user_text = req["messages"][0]["content"].as_str().unwrap_or_default();
            axum::Json(json!({
                "id": "msg_embed_999",
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "text",
                    "text": format!("SDK Anthropic Echo: {}", user_text)
                }],
                "model": "deepseek-v4-flash",
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 20
                }
            }))
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock_anthropic).await.unwrap();
    });

    let gateway = PonyGateway::builder()
        .add_provider(
            "deepseek-anthropic",
            format!("http://{}/anthropic", addr),
            "deepseek-v4-flash",
            RoutingStrategy::Priority,
        )
        .add_key("deepseek-anthropic", "ds-key-1", "sk-ds-key-1", 1, 10)
        .build();

    // Anthropic direct call
    let ant_req = MessageRequest {
        model: "deepseek-v4-flash".to_string(),
        messages: vec![AnthropicMessage {
            role: AnthropicRole::User,
            content: "Hello from direct SDK Anthropic".into(),
        }],
        max_tokens: 1024,
        ..Default::default()
    };

    let ant_resp = gateway.create_message(&ant_req).await.unwrap();
    assert_eq!(ant_resp.role, "assistant");
    assert_eq!(
        ant_resp.content[0],
        AnthropicContentBlock::Text {
            text: "SDK Anthropic Echo: Hello from direct SDK Anthropic".to_string(),
            cache_control: None,
        }
    );

    // Cross OpenAI Chat call translated to Anthropic upstream and back
    let chat_req = ChatCompletionRequest {
        model: "deepseek-v4-flash".to_string(),
        messages: vec![ChatMessage::User(UserMessage {
            content: "Hello from direct SDK Chat".into(),
            name: None,
        })],
        ..Default::default()
    };

    let chat_resp = gateway.chat_completion(&chat_req).await.unwrap();
    assert_eq!(
        chat_resp.choices[0].message.content.as_deref(),
        Some("SDK Anthropic Echo: Hello from direct SDK Chat")
    );
}
