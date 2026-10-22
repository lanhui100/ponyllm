//! Wave-11 tools — env=off isolation binary.
//!
//! `PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION` is PROCESS-global: tests that need a
//! value DIFFERENT from the rest of the suite live in a dedicated binary.
//! This file sets the emulation to `off` and contains only the tests that
//! rely on that value — no locks, no concurrency, single fixed env.

use axum::response::IntoResponse as _;
use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, RoutingStrategy, UpstreamProtocol};
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const CHALLENGE_OK: &str = r#"{"data":{"biz_code":0,"biz_data":{"challenge":{
"algorithm":"DeepSeekHashV1",
"challenge":"98ba09594cfd94513beeccb58fc56795f2a95e7285aafb58792bd8a2a69d329a",
"salt":"s000",
"difficulty":8,
"signature":"sig2",
"expireAt":1729666800000
}},"biz_msg":"ok"}}"#;
const SESSION_OK: &str = r#"{"data":{"biz_code":0,"biz_data":{"chat_session":{"id":"sess-mock-1"}},"biz_msg":"ok"}}"#;

struct Upstream {
    base: String,
    calls: Arc<Mutex<Vec<String>>>,
}

async fn spawn_upstream() -> Arc<Upstream> {
    use axum::http::HeaderMap;
    use axum::response::Response;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let up = Arc::new(Upstream {
        base: format!("http://{addr}"),
        calls: Arc::new(Mutex::new(Vec::new())),
    });
    let up_challenge = up.clone();
    let up_session = up.clone();
    let up_completion = up.clone();
    let app = axum::Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            axum::routing::post(move |_b: axum::Json<serde_json::Value>| {
                let up = up_challenge.clone();
                async move {
                    up.calls.lock().unwrap().push("challenge".to_string());
                    axum::Json(serde_json::from_str::<serde_json::Value>(CHALLENGE_OK).unwrap())
                        .into_response()
                }
            }),
        )
        .route(
            "/api/v0/chat_session/create",
            axum::routing::post(move || {
                let up = up_session.clone();
                async move {
                    up.calls.lock().unwrap().push("session".to_string());
                    axum::Json(serde_json::from_str::<serde_json::Value>(SESSION_OK).unwrap())
                        .into_response()
                }
            }),
        )
        .route(
            "/api/v0/chat/completion",
            axum::routing::post(move |_h: HeaderMap, _b: axum::Json<serde_json::Value>| {
                let up = up_completion.clone();
                async move {
                    up.calls.lock().unwrap().push("completion".to_string());
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(axum::body::Body::from(
                            "event: ready\ndata: {\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}\n\nevent: close\ndata: {\"close_reason\":\"success\"}\n\n",
                        ))
                        .unwrap()
                }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    up
}

async fn spawn_gateway(upstream: &str) -> String {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.providers.insert(
        "deepseek-web".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: upstream.to_string(),
            default_model: "deepseek-chat".to_string(),
            strategy: "priority".to_string(),
            billing_mode: BillingMode::Metered,
            input_price: 0.0,
            cached_price: 0.0,
            output_price: 0.0,
            models: vec!["deepseek-chat".to_string()],
            model_specs: vec![ModelSpec {
                name: "deepseek-chat".to_string(),
                priority: Some(10),
                context_window: "128K".to_string(),
                max_output: "4K".to_string(),
                ..Default::default()
            }],
            default_protocol: Some(UpstreamProtocol::DeepSeekWeb),
            ..Default::default()
        },
    );
    let pool = Arc::new(KeyPool::new("deepseek-web", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("web-key-1", "web-token-abc", 1, 10));
    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek-web", pool);
    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn tool(name: &str) -> Value {
    json!({"type": "function", "function": {"name": name, "description": "d", "parameters": {"type": "object"}}})
}

async fn chat(gw: &str, body: Value) -> (reqwest::StatusCode, String) {
    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&body)
        .send()
        .await
        .unwrap();
    (resp.status(), resp.text().await.unwrap_or_default())
}

/// Fixed env for THIS binary: emulation OFF (a different value than the main
/// tools suite, which leaves it unset). Never removed — the process exits.
fn emulation_off() {
    std::env::set_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION", "off");
}

// ---------------------------------------------------------------------------
// R-TOOL-FAIL-01 emulation disabled + tools -> 4xx (no silent 200)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_fail_01_disabled_emulation_4xx() {
    emulation_off();
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "w"}],
               "tools": [tool("get_weather")]}),
    )
    .await;
    assert!(
        !s.is_success() && s.as_u16() < 500,
        "R-TOOL-FAIL-01: env=off + tools -> explicit 4xx (never silent 200); got {s} {t}"
    );
    assert!(
        t.contains("tool_emulation_disabled") || t.to_lowercase().contains("emulation"),
        "R-TOOL-FAIL-01: explicit code/message; got {t}"
    );
}

// ---------------------------------------------------------------------------
// L2-AT: env=off + tools -> 4xx AND zero silent completion upstream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_tools_env_off_no_silent_completion() {
    emulation_off();
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "w"}],
               "tools": [tool("get_weather")]}),
    )
    .await;
    assert!(
        !s.is_success() && s.as_u16() < 500,
        "adv: env=off + tools -> 4xx (G-T3 must never surface as a tool-less 200); got {s} {t}"
    );
    assert!(
        up.calls.lock().unwrap().is_empty(),
        "adv: env=off + tools -> zero upstream requests; got {:?}",
        up.calls.lock().unwrap().clone()
    );
}