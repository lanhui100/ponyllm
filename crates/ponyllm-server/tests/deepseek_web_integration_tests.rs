//! deepseek-web gateway integration (T4, wave-9 contract DW-*).
//!
//! Full chain against a mock chat.deepseek.com upstream (axum, random port,
//! mirroring `request_routing_tests.rs:68-102`): the gateway must (a) resolve
//! the deepseek-web protocol from the base URL heuristic, (b) run the PoW
//! dance (create_pow_challenge → solve → `X-DS-PoW-Response`) plus
//! chat_session/create before the completion call, and (c) translate the
//! deepseek-web SSE events (`ready`/`delta`/`finish`/`close`) into OpenAI
//! chat-completion chunks. Owned by T4 (exec-integration): `deepseek_web_`
//! prefix per the T4 task card. The frozen T2 acceptance files are separate
//! and untouched.
//!
//! The mock challenge uses a worker-verified golden vector (see
//! `crates/ponyllm-core/src/deepseek_web/pow.rs` golden tests): salt `abc`,
//! signature `sig1`, expireAt 12345, difficulty 1 → answer `0` with the
//! digest below, so the gateway's `solve_pow` deterministically succeeds.

use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::json;
use std::sync::Arc;

fn model_spec(name: &str) -> ModelSpec {
    ModelSpec {
        rate_limits: None,
        priority: None,
        name: name.to_string(),
        tier: ModelTier::Standard,
        context_window: "128K".to_string(),
        max_output: "16K".to_string(),
        input_types: vec!["text".to_string()],
        output_types: vec!["text".to_string()],
        ..Default::default()
    }
}

/// Spin up a mock chat.deepseek.com upstream serving the three wave-9
/// endpoints: `create_pow_challenge`, `chat_session/create`, and the SSE
/// `completion`. Returns the base URL.
async fn spawn_mock_upstream() -> String {
    let challenge = json!({
        "biz_code": 0,
        "biz_data": {
            "challenge": {
                "algorithm": "DeepSeekHashV1",
                "challenge": "f8fb769c7e72e8c700bc813f7fea91949c91f96264d04cfe64063a76bd48fafb",
                "salt": "abc",
                "difficulty": 1,
                "signature": "sig1",
                "expireAt": 12345,
                "expireAfter": 3600
            }
        }
    });

    let app = Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            post(|Json(req): Json<serde_json::Value>| async move {
                assert_eq!(
                    req["target_path"],
                    "/api/v0/chat/completion",
                    "DW-S-01: challenge body must carry the completion target path"
                );
                Json(challenge)
            }),
        )
        .route(
            "/api/v0/chat_session/create",
            post(|Json(_req): Json<serde_json::Value>| async move {
                Json(json!({
                    "biz_code": 0,
                    "biz_data": { "chat_session": { "id": "sess-mock-1" } }
                }))
            }),
        )
        .route(
            "/api/v0/chat/completion",
            post(|Json(req): Json<serde_json::Value>| async move {
                // DW-Q-01 exact key set on the wire completion body.
                let keys: Vec<&str> = req
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(|k| k.as_str())
                    .collect();
                for expected in [
                    "chat_session_id",
                    "parent_message_id",
                    "model_type",
                    "prompt",
                    "ref_file_ids",
                    "thinking_enabled",
                    "search_enabled",
                    "source",
                    "action",
                    "preempt",
                ] {
                    assert!(keys.contains(&expected), "completion body missing {expected}");
                }
                assert_eq!(req["chat_session_id"], "sess-mock-1");
                assert_eq!(req["model_type"], "deepseek-v4-flash");
                assert_eq!(req["prompt"], "Hello");

                let body = format!(
                    "event: ready\ndata: {}\n\n\
                     event: delta\ndata: {}\n\n\
                     event: delta\ndata: {}\n\n\
                     event: finish\ndata: {}\n\n\
                     event: close\ndata: {}\n\n",
                    json!({"response_message_id": "rm-1", "request_message_id": "rq-1", "model_type": "deepseek-v4-flash"}),
                    json!({"o": "SET", "p": "/message/content/0/type", "v": "text"}),
                    json!({"o": "SET", "p": "/message/content/0/content", "v": "Hello from deepseek-web"}),
                    json!({"usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}}),
                    json!({"close_reason": "success"})
                );
                axum::response::Response::new(axum::body::Body::from(body))
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{}", addr)
}

fn provider_config(base_url: &str) -> ProviderConfig {
    ProviderConfig {
        egress_pool: vec![],
        egress_strategy: "round_robin".to_string(),
        rate_limits: None,
        base_url: base_url.to_string(),
        default_model: "deepseek-v4-flash".to_string(),
        strategy: "priority".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.14,
        cached_price: 0.014,
        output_price: 0.28,
        models: vec!["deepseek-v4-flash".to_string()],
        model_specs: vec![model_spec("deepseek-v4-flash")],
        default_protocol: Some(ponyllm_core::pool::UpstreamProtocol::DeepSeekWeb),
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
        timeout_secs: None,
        ttfb_timeout_secs: None,
    }
}

#[tokio::test]
async fn deepseek_web_full_chain_streams_translated_openai_chunks() {
    let upstream = spawn_mock_upstream().await;

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config
        .providers
        .insert("deepseek-web".to_string(), provider_config(&upstream));

    let pool = Arc::new(KeyPool::new("deepseek-web", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("ds-web-k1", "user-web-token-abc", 1, 10));
    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek-web", pool);

    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "messages": [{"role": "user", "content": "Hello"}],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "streaming chat must return 200");
    let body = resp.text().await.unwrap();

    // Parse `data: {…}` frames into values; every translated chunk must be a
    // chat.completion.chunk with the deepseek-web response id prefix.
    let chunks: Vec<serde_json::Value> = body
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter_map(|d| serde_json::from_str::<serde_json::Value>(d).ok())
        .filter(|v| v.get("object").and_then(|o| o.as_str()) == Some("chat.completion.chunk"))
        .collect();
    assert!(!chunks.is_empty(), "expected translated OpenAI chunks");

    let first = &chunks[0];
    assert!(
        first["id"].as_str().unwrap_or("").starts_with("chatcmpl-dsw-rm-1"),
        "DW-T-01: ready-derived id prefix, got {:?}",
        first["id"]
    );
    assert_eq!(first["choices"][0]["delta"]["role"], "assistant");

    // SET content fragment → OpenAI content delta (DW-T-02).
    let text: String = chunks
        .iter()
        .filter_map(|c| c["choices"][0]["delta"]["content"].as_str())
        .collect();
    assert_eq!(text, "Hello from deepseek-web");

    // finish → stop chunk + usage (DW-T-04).
    let final_chunk = chunks.last().unwrap();
    assert_eq!(final_chunk["choices"][0]["finish_reason"], "stop");
    assert_eq!(final_chunk["usage"]["prompt_tokens"], 10);
    assert_eq!(final_chunk["usage"]["total_tokens"], 15);
}

#[tokio::test]
async fn deepseek_web_resolves_from_chat_deepseek_com_base_url_heuristic() {
    // Zero-migration heuristic: a provider with no declared protocol but a
    // `chat.deepseek.com` base resolves to DeepSeekWeb (state.rs heuristic).
    let mut config = GatewayConfig::default();
    let mut p = provider_config("https://chat.deepseek.com");
    p.default_protocol = None;
    config.providers.insert("deepseek-web".to_string(), p);
    let state = Arc::new(AppState::new(config));

    use ponyllm_server::routes::models::ParsedRequestModel;
    let parsed = ParsedRequestModel::parse("deepseek-v4-flash");
    let targets = state
        .resolve_routed_targets(&parsed, None)
        .unwrap();
    let t = targets
        .iter()
        .find(|t| t.provider_name == "deepseek-web")
        .expect("deepseek-web provider must be routed");
    assert_eq!(
        t.upstream_protocol,
        ponyllm_core::pool::UpstreamProtocol::DeepSeekWeb
    );
    assert_eq!(
        t.deepseek_web_completion_url(),
        "https://chat.deepseek.com/api/v0/chat/completion"
    );
}

#[tokio::test]
async fn deepseek_web_error_mapping_40012_banned_surfaces_and_stops() {
    // DW-E-02: biz_code 40012 (USER_IS_BANNED) on the challenge endpoint must
    // surface as an upstream failure and must not silently retry into the
    // completion endpoint (the key is isolated).
    let app = Router::new().route(
        "/api/v0/chat/create_pow_challenge",
        post(|Json(_req): Json<serde_json::Value>| async move {
            Json(json!({
                "biz_code": 40012,
                "biz_msg": "USER_IS_BANNED"
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.providers.insert(
        "deepseek-web".to_string(),
        provider_config(&format!("http://{}", addr)),
    );
    let pool = Arc::new(KeyPool::new("deepseek-web", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("ds-web-k1", "user-web-token-abc", 1, 10));
    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek-web", pool);

    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/chat/completions", gw_addr))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "messages": [{"role": "user", "content": "Hello"}]
        }))
        .send()
        .await
        .unwrap();
    // Banned is permanent: the request fails (4xx/5xx), never a silent 200.
    assert!(
        resp.status().is_client_error() || resp.status().is_server_error(),
        "banned token must fail, got {}",
        resp.status()
    );
}
