//! Integration tests for the OpenAI Images API endpoints
//! (`/v1/images/generations`, `/v1/images/edits`) routing to an Antigravity
//! upstream via the `v1internal:generateContent` wire shape.

use std::sync::Arc;

use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::pool::*;
use ponyllm_server::config::{default_context_window, default_max_output};
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::json;

/// ProviderConfig with an image-output model on the Antigravity protocol.
fn image_provider(base_url: String) -> ProviderConfig {
    let mut spec = ModelSpec {
        name: "gemini-3.1-flash-image".to_string(),
        tier: ModelTier::Standard,
        context_window: default_context_window(),
        max_output: default_max_output(),
        input_types: vec!["text".to_string(), "image".to_string()],
        output_types: vec!["image".to_string()],
        protocol: Some(UpstreamProtocol::Antigravity),
        ..Default::default()
    };
    spec.protocol = Some(UpstreamProtocol::Antigravity);
    ProviderConfig {
        egress_pool: vec![],
        egress_strategy: "round_robin".to_string(),
        base_url,
        default_model: "gemini-3.1-flash-image".to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.0,
        cached_price: 0.0,
        output_price: 0.0,
        models: vec!["gemini-3.1-flash-image".to_string()],
        model_specs: vec![spec],
        default_protocol: Some(UpstreamProtocol::Antigravity),
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
        timeout_secs: None,
        ttfb_timeout_secs: None,
        rate_limits: None,
    }
}

fn antigravity_image_response(b64: &str) -> serde_json::Value {
    json!({
        "response": {
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [
                        {"thoughtSignature": "sig-1"},
                        {"inlineData": {"mimeType": "image/jpeg", "data": b64}}
                    ]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 12,
                "candidatesTokenCount": 9,
                "totalTokenCount": 21
            }
        }
    })
}

#[tokio::test]
async fn test_images_generations_openai_protocol() {
    let mock = Router::new().route(
        "/v1internal:generateContent",
        post(|Json(req): Json<serde_json::Value>| async move {
            assert_eq!(req["model"], "gemini-3.1-flash-image");
            assert_eq!(req["project"], "aicode-consumers");
            assert_eq!(req["requestType"], "agent");
            assert_eq!(
                req["request"]["contents"][0]["parts"][0]["text"],
                "a red apple"
            );
            assert_eq!(
                req["request"]["generationConfig"]["imageConfig"]["aspectRatio"], "1:1",
                "size=1024x1024 should map to aspectRatio 1:1, got: {}",
                req
            );
            Json(antigravity_image_response("SU5WRVJURUQ="))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("agy", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "agy".to_string(),
        image_provider(format!("http://{}", addr)),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("agy", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/images/generations", gw_addr))
        .json(&json!({
            "model": "gemini-3.1-flash-image",
            "prompt": "a red apple",
            "size": "1024x1024",
            "response_format": "b64_json"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "body: {}",
        resp.text().await.unwrap_or_default()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], serde_json::Value::Null); // OpenAI images has no object field
    assert_eq!(body["model"], "gemini-3.1-flash-image");
    assert_eq!(body["data"][0]["b64_json"], "SU5WRVJURUQ=");
    assert!(body["created"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn test_images_edits_json_wire() {
    let mock = Router::new().route(
        "/v1internal:generateContent",
        post(|Json(req): Json<serde_json::Value>| async move {
            let parts = &req["request"]["contents"][0]["parts"];
            assert_eq!(parts[0]["inlineData"]["mimeType"], "image/png");
            assert_eq!(parts[0]["inlineData"]["data"], "QUFB");
            assert_eq!(
                parts[1]["text"],
                "Based on the input image, modify it according to: make it purple"
            );
            Json(antigravity_image_response("RURJVEVE="))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("agy", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "agy".to_string(),
        image_provider(format!("http://{}", addr)),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("agy", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/images/edits", gw_addr))
        .json(&json!({
            "model": "gemini-3.1-flash-image",
            "prompt": "make it purple",
            "image": "data:image/png;base64,QUFB"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "body: {}",
        resp.text().await.unwrap_or_default()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"][0]["b64_json"], "RURJVEVE=");
    assert_eq!(body["model"], "gemini-3.1-flash-image");
}

#[tokio::test]
async fn test_images_edits_rejects_missing_image() {
    let pool = Arc::new(KeyPool::new("agy", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.providers.insert(
        "agy".to_string(),
        image_provider("http://127.0.0.1:9".to_string()),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("agy", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/images/edits", gw_addr))
        .json(&json!({
            "model": "gemini-3.1-flash-image",
            "prompt": "make it purple"
            // image missing!
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "missing_image");
}

#[tokio::test]
async fn test_images_edits_multipart_wire() {
    let mock = Router::new().route(
        "/v1internal:generateContent",
        post(|Json(req): Json<serde_json::Value>| async move {
            let parts = &req["request"]["contents"][0]["parts"];
            assert_eq!(parts[0]["inlineData"]["mimeType"], "image/jpeg");
            // base64 of "multipart-bytes"
            assert_eq!(parts[0]["inlineData"]["data"], "bXVsdGlwYXJ0LWJ5dGVz");
            assert_eq!(
                parts[1]["text"],
                "Based on the input image, modify it according to: add a halo"
            );
            Json(antigravity_image_response("TVVMVElQQVJU"))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("agy", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "agy".to_string(),
        image_provider(format!("http://{}", addr)),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("agy", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    // Hand-rolled multipart/form-data (workspace reqwest has no multipart feature).
    let boundary = "pony-test-boundary-42";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngemini-3.1-flash-image\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nadd a halo\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"input.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n\
         multipart-bytes\r\n\
         --{b}--\r\n",
        b = boundary
    );
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/images/edits", gw_addr))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "body: {}",
        resp.text().await.unwrap_or_default()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"][0]["b64_json"], "TVVMVElQQVJU");
}

#[tokio::test]
async fn test_images_generations_rejects_n_greater_than_one() {
    let pool = Arc::new(KeyPool::new("agy", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "agy".to_string(),
        image_provider("http://127.0.0.1:9".to_string()), // unreachable; must not be hit
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("agy", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/images/generations", gw_addr))
        .json(&json!({"model": "gemini-3.1-flash-image", "prompt": "a cat", "n": 2}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unsupported_n");
}

#[tokio::test]
async fn test_images_generations_rejects_non_image_output_model() {
    let pool = Arc::new(KeyPool::new("agy", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "agy".to_string(),
        // default ModelSpec output_types = ["text"] — must be rejected
        image_provider_with_text_output("http://127.0.0.1:9".to_string()),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("agy", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/images/generations", gw_addr))
        .json(&json!({"model": "gemini-3.1-flash-image", "prompt": "a cat"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unsupported_output_type");
}

fn image_provider_with_text_output(base_url: String) -> ProviderConfig {
    let mut p = image_provider(base_url);
    for spec in &mut p.model_specs {
        spec.output_types = vec!["text".to_string()];
    }
    p
}

#[tokio::test]
async fn test_images_generations_unknown_model_404() {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: behavior test opts into open mode
    let state = Arc::new(AppState::new(config));
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/images/generations", gw_addr))
        .json(&json!({"model": "no-such-image-model", "prompt": "a cat"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "model_not_found");
}

#[tokio::test]
async fn test_images_generations_upstream_error_projection() {
    let mock = Router::new().route(
        "/v1internal:generateContent",
        post(|| async {
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": {"code": 503, "message": "The model is overloaded"}})),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("agy", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-test", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open; // F1 migration: default is now secured; these behavior tests opt into open mode
    config.providers.insert(
        "agy".to_string(),
        image_provider(format!("http://{}", addr)),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("agy", pool);
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/images/generations", gw_addr))
        .json(&json!({"model": "gemini-3.1-flash-image", "prompt": "a cat"}))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_server_error(),
        "got status {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("exhausted"));
}

#[tokio::test]
async fn test_images_endpoints_require_auth_scope() {
    use ponyllm_config::{generate_scoped_gateway_key, AuthCompat};
    let (infer_plain, e_infer) =
        generate_scoped_gateway_key("agt-1", ponyllm_config::KeyScope::Inference);
    let mut config = GatewayConfig::default();
    config.auth_compat = AuthCompat::Strict;
    config.gateway_keys = vec![e_infer];
    let state = Arc::new(AppState::new(config));
    let app = create_app(state);
    let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    // Inference-scoped key must be authorized to reach the endpoint (routing
    // then fails with model_not_found — proving the auth layer let it through).
    let resp = client
        .post(format!("http://{}/v1/images/generations", gw_addr))
        .bearer_auth(&infer_plain)
        .json(&json!({"model": "nope", "prompt": "x"}))
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status(), 401, "inference key must authenticate");
    assert_ne!(
        resp.status(),
        403,
        "inference key must be authorized on images"
    );
}
