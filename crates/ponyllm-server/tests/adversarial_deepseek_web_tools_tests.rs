//! Wave-11 deepseek-web tools — L2-AT adversarial (red phase).
//!
//! Contract `.dev-team/contracts/wave-11-deepseek-web-tools.md`. Deterministic
//! postures: zero-upstream proofs, malformed-JSON variants, env-gate
//! no-silent-drop, schema non-leak. Every test is RED now (tool emulation
//! unimplemented -> tools silently dropped -> 200 bare concat) and must go
//! GREEN after task-10.

use axum::response::IntoResponse as _;
use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, RoutingStrategy, UpstreamProtocol};
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

static ENV_LOCK: Mutex<()> = Mutex::new(());
fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
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
    calls: Arc<Mutex<Vec<String>>>,
    completion_body: Arc<Mutex<Option<Value>>>,
}

async fn spawn_upstream(output_sse: String) -> Arc<Upstream> {
    use axum::http::HeaderMap;
    use axum::response::Response;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let up = Arc::new(Upstream {
        base: format!("http://{addr}"),
        calls: Arc::new(Mutex::new(Vec::new())),
        completion_body: Arc::new(Mutex::new(None)),
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
            axum::routing::post(move |_h: HeaderMap, body: axum::Json<serde_json::Value>| {
                let up = up_completion.clone();
                let sse = output_sse.clone();
                async move {
                    up.calls.lock().unwrap().push("completion".to_string());
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

fn tools_req(tools: Value, choice: Option<Value>) -> Value {
    let mut r = json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "w"}], "tools": tools});
    if let Some(c) = choice {
        r["tool_choice"] = c;
    }
    r
}

/// Post-filter: single_ok_call(body) -> the parsed tool_calls; passthrough_ok(body) -> content preserved.
fn tool_calls_of(body: &Value) -> Vec<Value> {
    body["choices"][0]["message"]["tool_calls"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// malformed JSON variants -> auto passthrough (no forged call, no 5xx)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_tools_malformed_json_variants() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let variants: [(&str, &str); 3] = [
        ("truncated", r#"{"tool_call":"get_weather","arguments":{"city":"Sha"#),
        ("nested-malformed", r#"{"tool_call":"get_weather","arguments":{"city":{"nested":[1,2,}}}"#),
        ("non-object", r#"[1,2,3]"#),
    ];
    for (label, out) in variants {
        let up = spawn_upstream(sse_with(out)).await;
        let gw = spawn_gateway(&up.base).await;
        let (s, t) = chat(
            &gw,
            tools_req(json!([tool("get_weather"), tool("get_time")]), None),
        )
        .await;
        assert!(
            s.is_success(),
            "adv: {label} auto parse failure -> 200 passthrough (never 5xx); got {s} {t}"
        );
        let body: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
        assert!(
            tool_calls_of(&body).is_empty(),
            "adv: {label}: no forged/partial tool_calls; got {t}"
        );
        let content = body["choices"][0]["message"]["content"].as_str().unwrap_or("");
        assert!(
            content.contains(out),
            "adv: {label}: original text passed through (not swallowed); got {t}"
        );
    }
}

// ---------------------------------------------------------------------------
// explanation-wrapped JSON: tolerant extraction OR clean passthrough — never a
// partial call, never a 5xx, never a silent content drop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_tools_explanation_wrapped_output() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let wrapped = r#"Okay! I will call the tool now: {"tool_call":"get_weather","arguments":{"city":"Shanghai"}} please wait"#;
    let up = spawn_upstream(sse_with(wrapped)).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(&gw, tools_req(json!([tool("get_weather")]), None)).await;
    assert!(s.is_success(), "adv: wrapped output never 5xx; got {s} {t}");
    let body: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
    let calls = tool_calls_of(&body);
    let content = body["choices"][0]["message"]["content"].as_str().unwrap_or("");
    let extracted_ok = calls.len() == 1
        && calls[0]["function"]["name"] == "get_weather"
        && content.find("tool_call").is_none(); // extraction may drop the wrapper
    let passthrough_ok = calls.is_empty() && content.contains("Okay! I will call the tool now");
    assert!(
        extracted_ok || passthrough_ok,
        "adv: wrapped output -> either one clean call OR clean passthrough (no partial/no silent drop); got {t}"
    );
}

// ---------------------------------------------------------------------------
// 65 tools / >64KB schema: 4xx + ZERO upstream requests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_tools_over_limits_zero_upstream() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    // 65 tools
    let up = spawn_upstream(sse_with("x")).await;
    let gw = spawn_gateway(&up.base).await;
    let many: Vec<Value> = (0..65).map(|i| tool(&format!("tool_{i}"))).collect();
    let (s, t) = chat(&gw, tools_req(json!(many), None)).await;
    assert!(
        !s.is_success() && s.as_u16() < 500,
        "adv: 65 tools -> 4xx; got {s} {t}"
    );
    assert!(
        up.calls.lock().unwrap().is_empty(),
        "adv: 65 tools -> zero upstream requests; got {:?}",
        up.snapshot_for_test()
    );
}

impl Upstream {
    fn snapshot_for_test(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn adv_tools_schema_over_64kb_zero_upstream() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let big = json!({"type": "object", "properties": {"x": {"type": "string", "description": "a".repeat(66000)}}});
    let up = spawn_upstream(sse_with("x")).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(
        &gw,
        tools_req(json!([{"type": "function", "function": {"name": "big", "description": "d", "parameters": big}}]), None),
    )
    .await;
    assert!(
        !s.is_success() && s.as_u16() < 500,
        "adv: >64KB schema -> 4xx; got {s} {t}"
    );
    assert!(
        up.snapshot_for_test().is_empty(),
        "adv: >64KB schema -> zero upstream requests; got {:?}",
        up.snapshot_for_test()
    );
}

// ---------------------------------------------------------------------------
// illegal tool_choice name -> 4xx + zero upstream (special-char variant)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_tools_illegal_choice_name_zero_upstream() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let up = spawn_upstream(sse_with("x")).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(
        &gw,
        tools_req(json!([tool("get_weather")]), Some(json!({"function": {"name": "bad name!"}}))),
    )
    .await;
    assert!(
        !s.is_success() && s.as_u16() < 500,
        "adv: illegal tool_choice name -> 4xx explicit; got {s} {t}"
    );
    assert!(
        up.snapshot_for_test().is_empty(),
        "adv: illegal name -> zero upstream requests; got {:?}",
        up.snapshot_for_test()
    );
}

// ---------------------------------------------------------------------------
// emulation off + tools -> 4xx AND the model never sees the request (no silent
// tool-less completion)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_tools_env_off_no_silent_completion() {
    let _guard = env_guard();
    std::env::set_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION", "OFF");
    let up = spawn_upstream(sse_with("I have no tools")).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(&gw, tools_req(json!([tool("get_weather")]), None)).await;
    assert!(
        !s.is_success() && s.as_u16() < 500,
        "adv: env=off + tools -> 4xx (G-T3 must never surface as a tool-less 200); got {s} {t}"
    );
    assert!(
        up.snapshot_for_test().is_empty(),
        "adv: env=off + tools -> zero upstream requests (no silent completion); got {:?}",
        up.snapshot_for_test()
    );
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
}

// ---------------------------------------------------------------------------
// schema never leaks into the streamed downstream body (raw-text check)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adv_tools_schema_not_in_streamed_body() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let up = spawn_upstream(sse_with("plain answer")).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(
        &gw,
        tools_req(json!([tool("get_weather")]), None),
    )
    .await;
    assert!(s.is_success(), "got {s} {t}");
    assert!(
        !t.contains("get_weather"),
        "adv: tool name/schema must not appear anywhere in the downstream body; got {t}"
    );
    assert!(t.contains("plain answer"), "adv: content intact; got {t}");
}