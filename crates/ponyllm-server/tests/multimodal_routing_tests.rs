use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, ModelTier, RoutingStrategy, UpstreamProtocol};
use ponyllm_protocol::openai::chat::ChatCompletionResponse;
use ponyllm_server::app::create_app;
use ponyllm_server::config::{GatewayConfig, ModelSpec, ProviderConfig};
use ponyllm_server::state::AppState;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn make_multimodal_provider(
    base_url: String,
    model: &str,
    proto: UpstreamProtocol,
    input_types: Vec<String>,
    tier: ModelTier,
) -> ProviderConfig {
    ProviderConfig {
        base_url,
        default_model: model.to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.1,
        cached_price: 0.01,
        output_price: 0.2,
        models: vec![model.to_string()],
        model_specs: vec![ModelSpec {
            name: model.to_string(),
            tier,
            input_types,
            ..Default::default()
        }],
        default_protocol: Some(proto),
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
    }
}

fn responses_mock_response(model: &str, text: &str) -> serde_json::Value {
    json!({
        "id": "resp-mock-mm",
        "object": "response",
        "status": "completed",
        "model": model,
        "output": [{
            "type": "message",
            "id": "msg-mm-1",
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "text", "text": text}]
        }],
        "usage": {"total_tokens": 50, "input_tokens": 40, "output_tokens": 10}
    })
}

#[tokio::test]
async fn test_chat_multimodal_to_responses_upstream_preserves_images() {
    let captured_input = Arc::new(parking_lot::Mutex::new(None::<serde_json::Value>));
    let captured_clone = Arc::clone(&captured_input);

    let mock = Router::new().route(
        "/v1/responses",
        post(move |Json(req): Json<serde_json::Value>| {
            let captured = Arc::clone(&captured_clone);
            async move {
                *captured.lock() = Some(req.clone());
                axum::Json(responses_mock_response(
                    req["model"].as_str().unwrap_or("muse-spark"),
                    "Image received successfully",
                ))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("spark_p", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.providers.insert(
        "spark_p".to_string(),
        make_multimodal_provider(
            format!("http://{}", addr),
            "muse-spark",
            UpstreamProtocol::Responses,
            vec!["text".to_string(), "image".to_string()],
            ModelTier::Standard,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("spark_p", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "muse-spark",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "What is in this picture?"},
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": "https://example.com/test.png",
                            "detail": "high"
                        }
                    }
                ]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let chat_resp: ChatCompletionResponse = resp.json().await.unwrap();
    assert_eq!(
        chat_resp.choices[0].message.content.as_deref(),
        Some("Image received successfully")
    );

    // Verify upstream received the image part intact, NOT dropped!
    let upstream_req = captured_input.lock().clone().expect("Upstream must receive request");
    let input_items = upstream_req["input"].as_array().expect("Input must be array of items");
    let user_msg = &input_items[0];
    let content_parts = user_msg["content"].as_array().expect("Content must be array of parts");
    
    let has_image_part = content_parts.iter().any(|part| {
        part.get("type").and_then(|t| t.as_str()) == Some("input_image")
            && part.get("image_url").and_then(|u| u.as_str()) == Some("https://example.com/test.png")
    });
    assert!(has_image_part, "Upstream request must preserve input_image part! Got: {}", upstream_req);
}

#[tokio::test]
async fn test_chat_multimodal_to_text_only_model_rejected_with_400() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&call_count);

    let mock = Router::new().route(
        "/v1/chat/completions",
        post(move |_body: String| {
            count_clone.fetch_add(1, Ordering::SeqCst);
            async move { "should not be called" }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("text_pool", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.providers.insert(
        "text_pool".to_string(),
        make_multimodal_provider(
            format!("http://{}", addr),
            "llama3-text",
            UpstreamProtocol::Chat,
            vec!["text".to_string()], // Text only!
            ModelTier::Standard,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("text_pool", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "llama3-text",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "Describe this image"},
                    {
                        "type": "image_url",
                        "image_url": {"url": "https://example.com/cat.png"}
                    }
                ]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    let err_body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(err_body["error"]["code"], "unsupported_modality");
    assert!(err_body["error"]["message"].as_str().unwrap().contains("does not support modality 'image'"));
    // Verify upstream was NOT hit at all
    assert_eq!(call_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_responses_multimodal_to_text_only_model_rejected_with_400() {
    let pool = Arc::new(KeyPool::new("text_pool2", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.providers.insert(
        "text_pool2".to_string(),
        make_multimodal_provider(
            "http://127.0.0.1:19999".to_string(),
            "text-model",
            UpstreamProtocol::Responses,
            vec!["text".to_string()],
            ModelTier::Standard,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("text_pool2", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", gw_addr))
        .json(&json!({
            "model": "text-model",
            "input": [{
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "Hello"},
                    {"type": "input_image", "image_url": "https://example.com/foo.jpg"}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    let err_body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(err_body["error"]["code"], "unsupported_modality");
    assert!(err_body["error"]["message"].as_str().unwrap().contains("does not support modality 'image'"));
}

#[tokio::test]
async fn test_messages_multimodal_to_responses_upstream_preserves_images() {
    let captured_input = Arc::new(parking_lot::Mutex::new(None::<serde_json::Value>));
    let captured_clone = Arc::clone(&captured_input);

    let mock = Router::new().route(
        "/v1/responses",
        post(move |Json(req): Json<serde_json::Value>| {
            let captured = Arc::clone(&captured_clone);
            async move {
                *captured.lock() = Some(req.clone());
                axum::Json(responses_mock_response(
                    req["model"].as_str().unwrap_or("resp-vlm"),
                    "Anthropic image transformed to responses cleanly",
                ))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("resp_vlm_pool", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.providers.insert(
        "resp_vlm_pool".to_string(),
        make_multimodal_provider(
            format!("http://{}", addr),
            "resp-vlm",
            UpstreamProtocol::Responses,
            vec!["text".to_string(), "image".to_string()],
            ModelTier::Flagship,
        ),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("resp_vlm_pool", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/messages", gw_addr))
        .json(&json!({
            "model": "resp-vlm",
            "max_tokens": 100,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "Explain this graphic"},
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/jpeg",
                            "data": "/9j/4AAQSkZJRgABAQ..."
                        }
                    }
                ]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["role"], "assistant");
    assert_eq!(body["content"][0]["text"], "Anthropic image transformed to responses cleanly");

    let upstream_req = captured_input.lock().clone().expect("Upstream must receive request");
    let input_items = upstream_req["input"].as_array().expect("Input must be array of items");
    let user_msg = &input_items[0];
    let content_parts = user_msg["content"].as_array().expect("Content must be array of parts");

    let has_image_part = content_parts.iter().any(|part| {
        part.get("type").and_then(|t| t.as_str()) == Some("input_image")
            && part.get("image_url").and_then(|u| u.as_str()).map(|s| s.starts_with("data:image/jpeg;base64,")).unwrap_or(false)
    });
    assert!(has_image_part, "Upstream request must preserve translated data URL image! Got: {}", upstream_req);
}

#[tokio::test]
async fn test_auto_routing_modality_awareness_and_tier_elevation() {
    let mock = Router::new()
        .route(
            "/v1/chat/completions",
            post(|Json(req): Json<serde_json::Value>| async move {
                let model = req["model"].as_str().unwrap_or("unknown");
                axum::Json(json!({
                    "id": "chat-mock",
                    "object": "chat.completion",
                    "created": 1234567,
                    "model": model,
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": format!("Answered by {}", model)},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
                }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool_std = Arc::new(KeyPool::new("prov_std", RoutingStrategy::RoundRobin));
    pool_std.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));

    let pool_flag = Arc::new(KeyPool::new("prov_flag", RoutingStrategy::RoundRobin));
    pool_flag.add_key(ApiKeyEntry::new("k2", "sk-test", 1, 10));

    let mut config = GatewayConfig::default();
    // Standard tier has ONLY text support
    config.providers.insert(
        "prov_std".to_string(),
        make_multimodal_provider(
            format!("http://{}", addr),
            "model-text-std",
            UpstreamProtocol::Chat,
            vec!["text".to_string()],
            ModelTier::Standard,
        ),
    );
    // Flagship tier has image multimodal support
    config.providers.insert(
        "prov_flag".to_string(),
        make_multimodal_provider(
            format!("http://{}", addr),
            "model-vision-flag",
            UpstreamProtocol::Chat,
            vec!["text".to_string(), "image".to_string()],
            ModelTier::Flagship,
        ),
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("prov_std", pool_std);
    state.register_pool("prov_flag", pool_flag);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // 1. Plain text auto request -> Should land on Standard tier (model-text-std)
    let resp_text = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "auto",
            "messages": [{"role": "user", "content": "Hello purely text"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp_text.status(), 200);
    assert_eq!(
        resp_text.headers().get("x-ponyllm-routed-model").unwrap().to_str().unwrap(),
        "model-text-std"
    );

    // 2. Multimodal image auto request -> Standard lacks image support, MUST automatically elevate to Flagship!
    let resp_img = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "auto",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "Analyze image"},
                    {"type": "image_url", "image_url": {"url": "https://example.com/pic.png"}}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp_img.status(), 200);
    assert_eq!(
        resp_img.headers().get("x-ponyllm-routed-model").unwrap().to_str().unwrap(),
        "model-vision-flag"
    );

    // 3. Multimodal image explicitly requesting standard tier (auto:standard) -> Cannot elevate, must return 400!
    let resp_pinned_tier = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "auto:standard",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "Analyze image"},
                    {"type": "image_url", "image_url": {"url": "https://example.com/pic.png"}}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp_pinned_tier.status(), 400);
    let err_json: serde_json::Value = resp_pinned_tier.json().await.unwrap();
    assert_eq!(err_json["error"]["code"], "unsupported_modality");
}
