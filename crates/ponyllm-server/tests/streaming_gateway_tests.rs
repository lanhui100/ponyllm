use std::sync::Arc;
use axum::body::Body;
use axum::http::HeaderValue;
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use serde_json::json;

fn make_provider(base_url: &str, default_model: &str) -> ProviderConfig {
    ProviderConfig {
        base_url: base_url.to_string(),
        default_model: default_model.to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.50,
        cached_price: 0.25,
        output_price: 1.00,
        models: vec![],
        model_specs: Vec::new(),
    }
}

fn sse_response(text: &'static str) -> Response {
    let mut resp = Response::new(Body::from(text));
    resp.headers_mut()
        .insert("content-type", HeaderValue::from_static("text/event-stream"));
    resp
}

/// Start a mock OpenAI upstream whose /v1/chat/completions returns SSE chunks,
/// and a gateway wired to it. Returns the gateway base URL.
async fn spawn_gateway_with_upstream(mock: Router) -> String {
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("deepseek", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-mock-key-123456", 1, 10));

    let mut config = GatewayConfig::default();
    config.providers.insert(
        "deepseek".to_string(),
        make_provider(&format!("http://{}", upstream_addr), "deepseek-v4-flash"),
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek", pool);

    let gateway_app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway_app).await.unwrap();
    });
    format!("http://{}", gateway_addr)
}

#[tokio::test]
async fn test_chat_streaming_no_double_data_prefix() {
    // Upstream sends correct OpenAI SSE (already `data:` prefixed).
    let mock = Router::new().route(
        "/v1/chat/completions",
        post(|_: Json<serde_json::Value>| async {
            sse_response(
                "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\ndata: [DONE]\n\n",
            )
        }),
    );
    let base = spawn_gateway_with_upstream(mock).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/chat/completions", base))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap().to_str().unwrap(),
        "text/event-stream"
    );
    let body = resp.text().await.unwrap();

    // No double prefix (regression for `data: data: {...}`)
    assert!(!body.contains("data: data:"), "double data prefix: {body}");
    assert!(body.contains("data: {\"id\":\"1\""), "missing chunk: {body}");
    assert!(body.ends_with("data: [DONE]\n\n"), "missing [DONE] terminator: {body}");
}

#[tokio::test]
async fn test_messages_streaming_translated_to_anthropic_events() {
    // Upstream is OpenAI-compatible: returns OpenAI chat chunks.
    let mock = Router::new().route(
        "/v1/chat/completions",
        post(|_: Json<serde_json::Value>| async {
            sse_response(
                "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":null},\"finish_reason\":null}]}\n\ndata: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n\ndata: [DONE]\n\n",
            )
        }),
    );
    let base = spawn_gateway_with_upstream(mock).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/messages", base))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "stream": true,
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    // Must be Anthropic SSE events: `event: message_start` / content_block_delta / message_stop
    assert!(body.contains("event: message_start"), "missing message_start: {body}");
    assert!(body.contains("event: content_block_delta"), "missing content_block_delta: {body}");
    assert!(body.contains("event: message_stop"), "missing message_stop: {body}");
    // Never leak raw OpenAI framing
    assert!(!body.contains("chat.completion.chunk"), "leaked openai chunk: {body}");
}

#[tokio::test]
async fn test_responses_virtual_model_mapped_to_physical() {
    // Upstream /v1/responses inspects the model it received.
    let received = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let received_clone = received.clone();
    let mock = Router::new().route(
        "/v1/responses",
        post(|Json(req): Json<serde_json::Value>| async move {
            *received_clone.lock().unwrap() = req["model"].as_str().unwrap_or("").to_string();
            Json(json!({
                "id": "resp-1",
                "object": "response",
                "created_at": 1710000000,
                "status": "completed",
                "model": req["model"],
                "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "ok"}]}]
            }))
        }),
    );
    let base = spawn_gateway_with_upstream(mock).await;
    let client = reqwest::Client::new();

    // Request via virtual model `auto` — gateway must map to physical model upstream.
    let resp = client
        .post(format!("{}/v1/responses", base))
        .json(&json!({"model": "auto", "input": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    // Routing headers present (regression: responses used to omit them)
    assert_eq!(resp.headers().get("x-ponyllm-routed-model").unwrap().to_str().unwrap(), "deepseek-v4-flash");
    assert_eq!(resp.headers().get("x-ponyllm-provider").unwrap().to_str().unwrap(), "deepseek");
    let body: serde_json::Value = resp.json().await.unwrap();
    // Model echo rule: response body echoes requested name
    assert_eq!(body["model"], "auto");

    let sent = received.lock().unwrap().clone();
    assert_eq!(sent, "deepseek-v4-flash", "upstream must receive physical model, got: {sent}");
}

#[tokio::test]
async fn test_empty_messages_rejected_with_400() {
    let mock = Router::new().route(
        "/v1/chat/completions",
        post(|_: Json<serde_json::Value>| async { Json(json!({"ok": true})) }),
    );
    let base = spawn_gateway_with_upstream(mock).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/chat/completions", base))
        .json(&json!({"model": "deepseek-v4-flash", "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let resp2 = client
        .post(format!("{}/v1/messages", base))
        .json(&json!({"model": "deepseek-v4-flash", "max_tokens": 10, "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 400);

    let resp3 = client
        .post(format!("{}/v1/responses", base))
        .json(&json!({"model": "deepseek-v4-flash", "input": ""}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp3.status(), 400);
}

#[tokio::test]
async fn test_unknown_model_returns_404() {
    let mock = Router::new().route(
        "/v1/chat/completions",
        post(|_: Json<serde_json::Value>| async { Json(json!({"ok": true})) }),
    );
    let base = spawn_gateway_with_upstream(mock).await;
    let client = reqwest::Client::new();

    // 1. Chat completions unknown model -> 404
    let resp_chat = client
        .post(format!("{}/v1/chat/completions", base))
        .json(&json!({
            "model": "non-existent-model-xyz",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_chat.status(), 404);
    let chat_err: serde_json::Value = resp_chat.json().await.unwrap();
    assert_eq!(chat_err["error"]["code"], "model_not_found");

    // 2. Messages unknown model -> 404 with Anthropic error framing
    let resp_msg = client
        .post(format!("{}/v1/messages", base))
        .json(&json!({
            "model": "non-existent-model-xyz",
            "max_tokens": 10,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_msg.status(), 404);
    let msg_err: serde_json::Value = resp_msg.json().await.unwrap();
    assert_eq!(msg_err["type"], "error");
    assert_eq!(msg_err["error"]["type"], "not_found_error");

    // 3. Responses unknown model -> 404
    let resp_resp = client
        .post(format!("{}/v1/responses", base))
        .json(&json!({
            "model": "non-existent-model-xyz",
            "input": "hi"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_resp.status(), 404);
}

#[tokio::test]
async fn test_malformed_json_returns_standard_json_error() {
    let mock = Router::new().route(
        "/v1/chat/completions",
        post(|_: Json<serde_json::Value>| async { Json(json!({"ok": true})) }),
    );
    let base = spawn_gateway_with_upstream(mock).await;
    let client = reqwest::Client::new();

    // 1. OpenAI chat with invalid JSON syntax -> standard JSON error (not text/plain)
    let resp_chat = client
        .post(format!("{}/v1/chat/completions", base))
        .header("content-type", "application/json")
        .body("{invalid-json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp_chat.status(), 400);
    assert_eq!(
        resp_chat.headers().get("content-type").unwrap().to_str().unwrap(),
        "application/json"
    );
    let chat_err: serde_json::Value = resp_chat.json().await.unwrap();
    assert_eq!(chat_err["error"]["type"], "invalid_request_error");

    // 2. Anthropic messages with invalid JSON -> Anthropic error object
    let resp_msg = client
        .post(format!("{}/v1/messages", base))
        .header("content-type", "application/json")
        .body("{invalid-json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp_msg.status(), 400);
    assert_eq!(
        resp_msg.headers().get("content-type").unwrap().to_str().unwrap(),
        "application/json"
    );
    let msg_err: serde_json::Value = resp_msg.json().await.unwrap();
    assert_eq!(msg_err["type"], "error");
    assert_eq!(msg_err["error"]["type"], "invalid_request_error");
}
