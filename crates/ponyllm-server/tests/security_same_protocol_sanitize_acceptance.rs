//! FROZEN RED-PHASE ACCEPTANCE — Wave-1 addendum (Lead ruling ④): the
//! SAME-PROTOCOL passthrough path must sanitize `extra` before serializing.
//!
//! A `/v1/chat/completions` request whose upstream protocol is also Chat is not
//! translated — the gateway serializes the (mutated) `ChatCompletionRequest`
//! straight to the wire. Because that struct carries
//! `#[serde(flatten)] extra: HashMap<String, Value>`, a client key that is NOT
//! a Chat field (e.g. the Responses-only `max_output_tokens: 999999`) is
//! re-emitted verbatim onto the upstream body. Contract C1/C2 requires the
//! same-protocol path to run the reserved-name sanitizer too.
//!
//! Assertion shape: the mock upstream records the exact JSON body it received;
//! the test asserts the hostile key/value is absent AND that a legal extension
//! key survives (over-tightening guard). No smoke-only assertions.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::pool::*;
use ponyllm_server::config::{default_context_window, default_max_output, default_modalities};
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::{json, Value};

/// Start a mock Chat upstream that records every request body.
async fn spawn_chat_upstream() -> (String, Arc<Mutex<Vec<Value>>>) {
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |_headers: axum::http::HeaderMap, body: String| {
            let sink = sink.clone();
            async move {
                let value: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                sink.lock().expect("upstream capture lock").push(value);
                Json(json!({
                    "id": "chatcmpl-capture",
                    "object": "chat.completion",
                    "created": 1,
                    "model": "gpt-4o-mini",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "ok"},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 3, "completion_tokens": 1, "total_tokens": 4}
                }))
                .into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), seen)
}

/// Gateway whose only provider speaks Chat upstream → same-protocol passthrough.
async fn spawn_gateway(upstream_base: String) -> std::net::SocketAddr {
    let pool = Arc::new(KeyPool::new("openai", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("oa-k1", "sk-test-oa", 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: upstream_base,
            default_model: "gpt-4o-mini".to_string(),
            strategy: "round_robin".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.15,
            cached_price: 0.075,
            output_price: 0.60,
            models: vec!["gpt-4o-mini".to_string()],
            model_specs: vec![ModelSpec {
                rate_limits: None,
                priority: None,
                name: "gpt-4o-mini".to_string(),
                tier: ModelTier::Standard,
                context_window: default_context_window(),
                max_output: default_max_output(),
                input_types: default_modalities(),
                output_types: default_modalities(),
                ..Default::default()
            }],
            default_protocol: Some(ponyllm_core::pool::UpstreamProtocol::Chat),
            chat_url: None,
            responses_url: None,
            messages_url: None,
            proxy: None,
            timeout_secs: None,
            ttfb_timeout_secs: None,
        },
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("openai", pool);
    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn same_protocol_chat_drops_cross_protocol_reserved_key() {
    let (upstream, seen) = spawn_chat_upstream().await;
    let gateway = spawn_gateway(upstream).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("client");
    let resp = client
        .post(format!("http://{gateway}/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 64,
            // Responses-only field smuggled onto a Chat body:
            "max_output_tokens": 999999,
            // Legal extension key that must survive (over-tightening guard):
            "x_provider_ext_flag": true
        }))
        .send()
        .await
        .expect("gateway request must complete");

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "gateway must serve the request successfully; body = {:?}",
        resp.text().await
    );

    let bodies = seen.lock().expect("upstream capture lock").clone();
    assert_eq!(
        bodies.len(),
        1,
        "exactly one upstream attempt expected, got {}",
        bodies.len()
    );
    let wire = &bodies[0];

    // C1/C2: the cross-protocol reserved key must not reach the upstream body.
    assert!(
        wire.get("max_output_tokens").is_none(),
        "same-protocol Chat path must strip the cross-protocol reserved key `max_output_tokens`; upstream body = {wire}"
    );
    assert!(
        !wire.to_string().contains("999999"),
        "same-protocol Chat path must not forward the injected value; upstream body = {wire}"
    );
    // The gateway's own field is untouched by the sanitization.
    assert_eq!(
        wire.get("max_tokens").and_then(Value::as_u64),
        Some(64),
        "the Chat-native `max_tokens` field must be forwarded unchanged; upstream body = {wire}"
    );
    // Over-tightening guard: a legal extension key survives the same path.
    assert_eq!(
        wire.get("x_provider_ext_flag"),
        Some(&json!(true)),
        "a legal extension key must still reach the upstream body; upstream body = {wire}"
    );
}
