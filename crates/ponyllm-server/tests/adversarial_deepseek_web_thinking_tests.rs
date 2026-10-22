//! Wave-12 thinking — L2-AT adversarial (red phase).
//!
//! Malformed top-level `thinking` shapes, oversized history reasoning, and the
//! 21-reasoning-frame stream ordering (gap matrix #4). Deterministic, no
//! env-dependence (default 4096 budget).

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
    completion_body: Arc<Mutex<Option<Value>>>,
}

async fn spawn_upstream(output_sse: String) -> Arc<Upstream> {
    use axum::http::HeaderMap;
    use axum::response::Response;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let up = Arc::new(Upstream {
        base: format!("http://{addr}"),
        completion_body: Arc::new(Mutex::new(None)),
    });
    let up_completion = up.clone();
    let app = axum::Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            axum::routing::post(|_b: axum::Json<serde_json::Value>| async move {
                axum::Json(serde_json::from_str::<serde_json::Value>(CHALLENGE_OK).unwrap())
                    .into_response()
            }),
        )
        .route(
            "/api/v0/chat_session/create",
            axum::routing::post(|| async move {
                axum::Json(serde_json::from_str::<serde_json::Value>(SESSION_OK).unwrap())
                    .into_response()
            }),
        )
        .route(
            "/api/v0/chat/completion",
            axum::routing::post(move |_h: HeaderMap, body: axum::Json<serde_json::Value>| {
                let up = up_completion.clone();
                let sse = output_sse.clone();
                async move {
                    *up.completion_body.lock().unwrap() = Some(body.0);
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(axum::body::Body::from(sse))
                        .unwrap()
                }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    up
}

fn sse_with(content: &str) -> String {
    let escaped = serde_json::to_string(content).unwrap();
    format!(
        "event: ready\ndata: {{\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}}\n\n\
event: delta\ndata: {{\"o\":\"SET\",\"p\":\"/message/content/0/content/0/text\",\"v\":{escaped}}}\n\n\
event: finish\ndata: {{\"response_message_id\":\"r1\"}}\n\n\
event: close\ndata: {{\"close_reason\":\"success\"}}\n\n"
    )
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

async fn chat_body(gw: &str, body: Value) -> (reqwest::StatusCode, String) {
    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&body)
        .send()
        .await
        .unwrap();
    (resp.status(), resp.text().await.unwrap_or_default())
}

fn thinking_of(up: &Arc<Upstream>) -> Option<bool> {
    up.completion_body
        .lock()
        .unwrap()
        .clone()
        .and_then(|b| b.get("thinking_enabled").and_then(|v| v.as_bool()))
}

// ---------------------------------------------------------------------------
// malformed top-level thinking shapes: ignored (no error), other sources stay
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_thinking_malformed_top_level_shapes() {
    for (label, thinking) in [
        ("string", json!("disabled")),
        ("array", json!(["disabled"])),
        ("number", json!(42)),
        ("null", Value::Null),
    ] {
        let up = spawn_upstream(sse_with("x")).await;
        let gw = spawn_gateway(&up.base).await;
        let body = json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "hi"}], "thinking": thinking});
        let (s, t) = chat_body(&gw, body).await;
        assert!(s.is_success(), "adv: {label} thinking ignored, no error; got {s} {t}");
        assert_eq!(
            thinking_of(&up),
            Some(true),
            "adv: {label} -> falls back to default On; got {:?}",
            thinking_of(&up)
        );
    }
}

// ---------------------------------------------------------------------------
// oversized history reasoning (100KB): truncated at default 4096 with marker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_thinking_huge_history_reasoning_truncated() {
    let up = spawn_upstream(sse_with("x")).await;
    let gw = spawn_gateway(&up.base).await;
    let body = json!({"model": "deepseek-chat", "messages": [
        {"role": "user", "content": "q"},
        {"role": "assistant", "content": "a", "reasoning_content": "w".repeat(100_000)}
    ]});
    let (s, t) = chat_body(&gw, body).await;
    assert!(s.is_success(), "got {s} {t}");
    let prompt = up.completion_body.lock().unwrap().clone().expect("body")
        .get("prompt").and_then(|p| p.as_str()).unwrap_or_default().to_string();
    let seg_start = prompt.find("[assistant-reasoning]").expect("segment") + "[assistant-reasoning]".len();
    let seg_end = prompt[seg_start..].find("[assistant]").map(|i| seg_start + i).unwrap_or(prompt.len());
    let content = prompt[seg_start..seg_end].trim_start_matches(|c| c == '\n' || c == '\r');
    let marker = "…(truncated)";
    let core = content.trim_end_matches(|c| c == '\n' || c == '\r');
    assert!(
        core.ends_with(marker) && core.len() - marker.len() <= 4096,
        "adv: 100KB reasoning truncated to <=4096 (excl. marker) with marker; got {}",
        core.len() - marker.len()
    );
}

// ---------------------------------------------------------------------------
// 21 reasoning frames then content: aggregated reasoning before content
// (gap matrix #4 shape)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_thinking_stream_21_reasoning_frames_then_content() {
    let mut frames = String::from(
        "event: ready\ndata: {\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}\n\n",
    );
    for i in 0..21 {
        frames.push_str(&format!(
            "event: delta\ndata: {{\"o\":\"SET\",\"p\":\"/message/content/0/content/0/type\",\"v\":\"thinking\"}}\n\nevent: delta\ndata: {{\"o\":\"SET\",\"p\":\"/message/content/0/content/0/text\",\"v\":\"frame {i}\"}}\n\n"
        ));
    }
    frames.push_str(
        "event: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/content/0/content/1/type\",\"v\":\"text\"}\n\nevent: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/content/0/content/1/text\",\"v\":\"FINAL\"}\n\n\
event: finish\ndata: {\"response_message_id\":\"r1\"}\n\nevent: close\ndata: {\"close_reason\":\"success\"}\n\n",
    );
    let up = spawn_upstream(frames).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat_body(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "q"}]}),
    )
    .await;
    assert!(s.is_success(), "got {s} {t}");
    let body: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
    let msg = &body["choices"][0]["message"];
    let reasoning = msg["reasoning_content"].as_str().unwrap_or("");
    let content = msg["content"].as_str().unwrap_or("");
    for i in 0..21 {
        assert!(
            reasoning.contains(&format!("frame {i}")),
            "adv: all 21 reasoning frames present; missing frame {i}"
        );
    }
    assert!(
        !reasoning.contains("FINAL") && content.contains("FINAL"),
        "adv: reasoning-only reasoning; content-only content; got {t}"
    );
}