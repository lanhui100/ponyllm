//! Wave-12 thinking — env=illegal isolation binary.
//!
//! `PONYLLM_WEB_REASONING_HISTORY_MAX_BYTES` set to a NON-NUMERIC value ONCE.
//! Per R-THINK-06: an illegal value must fall back to the 4096 default (tail
//! kept + `…(truncated)` marker on an over-4096 reasoning).

use axum::response::IntoResponse as _;
use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, RoutingStrategy, UpstreamProtocol};
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};

static ENV_ILLEGAL_ONCE: Once = Once::new();
fn ensure_env_illegal() {
    ENV_ILLEGAL_ONCE.call_once(|| {
        std::env::set_var("PONYLLM_WEB_REASONING_HISTORY_MAX_BYTES", "not-a-number");
    });
}

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

async fn spawn_upstream() -> Arc<Upstream> {
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
                async move {
                    *up.completion_body.lock().unwrap() = Some(body.0);
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

fn prompt_of(up: &Arc<Upstream>) -> String {
    up.completion_body
        .lock()
        .unwrap()
        .clone()
        .expect("completion body")
        .get("prompt")
        .and_then(|p| p.as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn r_think_06_env_illegal_falls_back_to_4096() {
    ensure_env_illegal();
    let up = spawn_upstream().await;
    let gw = spawn_gateway(&up.base).await;
    let body = json!({"model": "deepseek-chat", "messages": [
        {"role": "user", "content": "q"},
        {"role": "assistant", "content": "a", "reasoning_content": "z".repeat(8192)}
    ]});
    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let prompt = prompt_of(&up);
    let seg_start = prompt.find("[assistant-reasoning]").expect("segment present") + "[assistant-reasoning]".len();
    let seg_end = prompt[seg_start..].find("[assistant]").map(|i| seg_start + i).unwrap_or(prompt.len());
    let content = prompt[seg_start..seg_end].trim_start_matches(|c| c == '\n' || c == '\r');
    let marker = "…(truncated)";
    let core = content.trim_end_matches(|c| c == '\n' || c == '\r');
    assert!(
        core.ends_with(marker),
        "illegal env -> 4096 default truncation with marker; got tail: {}",
        &core[core.len().saturating_sub(30)..]
    );
    assert!(
        core.len() - marker.len() <= 4096,
        "illegal env -> falls back to 4096; got {}",
        core.len() - marker.len()
    );
}