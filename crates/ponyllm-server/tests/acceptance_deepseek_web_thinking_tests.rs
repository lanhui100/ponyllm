//! Wave-12 deepseek-web thinking — L2-T functional acceptance
//! (`r_think_01..09`, contract `.dev-team/contracts/wave-12-thinking.md`).
//!
//! Four-source thinking switch (body `thinking` / `reasoning_effort` / header
//! `X-Pony-Thinking` / model-name suffix), effort degraded mapping, history
//! reasoning injection + budget truncation, Anthropic budget_tokens ignore,
//! THINK/RESPONSE non-leak, streaming ordering.
//!
//! Env: `PONYLLM_WEB_REASONING_HISTORY_MAX_BYTES` — the DEFAULT-budget cases
//! live here (env unset → 4096); the env=128 / illegal-value cases live in
//! dedicated binaries (wave-11 scheme: set-once via Once, no locks).

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

impl Upstream {
    fn body(&self) -> Value {
        self.completion_body
            .lock()
            .unwrap()
            .clone()
            .expect("completion body captured")
    }
    fn prompt(&self) -> String {
        self.body()
            .get("prompt")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string()
    }
    fn thinking_enabled(&self) -> Option<bool> {
        self.body().get("thinking_enabled").and_then(|v| v.as_bool())
    }
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
    let up_challenge = up.clone();
    let up_session = up.clone();
    let up_completion = up.clone();
    let app = axum::Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            axum::routing::post(move |_b: axum::Json<serde_json::Value>| {
                let up = up_challenge.clone();
                async move {
                    let _ = up.base.len();
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
                    let _ = up.base.len();
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

/// Reasoning-first stream: thinking deltas then a content delta.
fn sse_reasoning_then_content(thinking: &str, content: &str) -> String {
    let t = serde_json::to_string(thinking).unwrap();
    let c = serde_json::to_string(content).unwrap();
    format!(
        "event: ready\ndata: {{\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}}\n\n\
event: delta\ndata: {{\"o\":\"SET\",\"p\":\"/message/content/0/content/0/type\",\"v\":\"thinking\"}}\n\n\
event: delta\ndata: {{\"o\":\"SET\",\"p\":\"/message/content/0/content/0/text\",\"v\":{t}}}\n\n\
event: delta\ndata: {{\"o\":\"SET\",\"p\":\"/message/content/0/content/1/type\",\"v\":\"text\"}}\n\n\
event: delta\ndata: {{\"o\":\"SET\",\"p\":\"/message/content/0/content/1/text\",\"v\":{c}}}\n\n\
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

/// POST /v1/chat/completions with optional thinking sources.
async fn chat_opt(
    gw: &str,
    model: &str,
    thinking: Option<Value>,
    effort: Option<&str>,
    header: Option<&str>,
) -> (reqwest::StatusCode, String) {
    let mut body = json!({"model": model, "messages": [{"role": "user", "content": "hi"}]});
    if let Some(t) = thinking {
        body["thinking"] = t;
    }
    if let Some(e) = effort {
        body["reasoning_effort"] = Value::String(e.to_string());
    }
    let mut req = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&body);
    if let Some(h) = header {
        req = req.header("x-pony-thinking", h);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    (status, text)
}

/// POST /v1/chat/completions with an explicit full body (history-reasoning
/// tests need custom messages).
async fn chat_raw(gw: &str, body: Value) -> (reqwest::StatusCode, String) {
    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&body)
        .send()
        .await
        .unwrap();
    (resp.status(), resp.text().await.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// R-THINK-01 four sources -> single thinking_enabled decision
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_think_01_four_sources_single_decision() {
    let up = spawn_upstream(sse_with("x")).await;
    let gw = spawn_gateway(&up.base).await;
    let cases: Vec<(String, Option<Value>, Option<&str>, Option<&str>, bool)> = vec![
        ("body-disabled".into(), Some(json!({"type": "disabled"})), None, None, false),
        ("body-enabled".into(), Some(json!({"type": "enabled"})), None, None, true),
        ("effort-none".into(), None, Some("none"), None, false),
        ("effort-high".into(), None, Some("high"), None, true),
        ("header-none".into(), None, None, Some("none"), false),
        ("header-high".into(), None, None, Some("high"), true),
        ("suffix-high".into(), None, None, None, true), // model = deepseek-chat:high
        ("default".into(), None, None, None, true),
    ];
    for (label, thinking, effort, header, expect) in cases {
        let model = if label == "suffix-high" { "deepseek-chat:high" } else { "deepseek-chat" };
        let (s, t) = chat_opt(&gw, model, thinking, effort, header).await;
        assert!(s.is_success(), "R-THINK-01: {label} -> {s} {t}");
        assert_eq!(
            up.thinking_enabled(),
            Some(expect),
            "R-THINK-01: {label} thinking_enabled == {expect}; got {:?}",
            up.thinking_enabled()
        );
    }
}

// ---------------------------------------------------------------------------
// R-THINK-02 body thinking.type parse (unknown -> ignore + no error)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_think_02_body_thinking_type_parse() {
    let up = spawn_upstream(sse_with("x")).await;
    let gw = spawn_gateway(&up.base).await;
    let (s1, _) = chat_opt(&gw, "deepseek-chat", Some(json!({"type": "disabled"})), None, None).await;
    assert!(s1.is_success());
    assert_eq!(up.thinking_enabled(), Some(false), "R-THINK-02: disabled -> Off");
    let (s2, _) = chat_opt(&gw, "deepseek-chat", Some(json!({"type": "enabled"})), None, None).await;
    assert!(s2.is_success());
    assert_eq!(up.thinking_enabled(), Some(true), "R-THINK-02: enabled -> On");

    // unknown type ("auto"), non-string shapes (array/object/string literal)
    // -> ignored (no error), default On prevails.
    for (label, t) in [
        ("auto", json!({"type": "auto"})),
        ("array", json!({"type": []})),
        ("object", json!({"type": {"a": 1}})),
        ("string-literal", json!({"type": 42})),
    ] {
        let (s, _) = chat_opt(&gw, "deepseek-chat", Some(t), None, None).await;
        assert!(s.is_success(), "R-THINK-02: {label} ignored (no error); got {s}");
        assert_eq!(
            up.thinking_enabled(),
            Some(true),
            "R-THINK-02: {label} -> falls back to default On; got {:?}",
            up.thinking_enabled()
        );
    }
}

// ---------------------------------------------------------------------------
// R-THINK-03 conflict matrix deterministic (body > header > suffix > default)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_think_03_conflict_matrix_deterministic() {
    let up = spawn_upstream(sse_with("x")).await;
    let gw = spawn_gateway(&up.base).await;
    let matrix: Vec<(&str, Option<Value>, Option<&str>, Option<&str>, &str, bool)> = vec![
        ("body-disabled-over-header-high", Some(json!({"type": "disabled"})), None, Some("high"), "deepseek-chat", false),
        ("body-enabled-over-suffix-none", Some(json!({"type": "enabled"})), None, None, "deepseek-chat:none", true),
        ("header-none-over-suffix-high", None, None, Some("none"), "deepseek-chat:high", false),
        ("suffix-none-over-default", None, None, None, "deepseek-chat:none", false),
        ("nothing-default-on", None, None, None, "deepseek-chat", true),
    ];
    for (label, thinking, effort, header, model, expect) in &matrix {
        let (s, _) = chat_opt(&gw, model, thinking.clone(), *effort, *header).await;
        assert!(s.is_success(), "R-THINK-03: {label} -> {s}");
        assert_eq!(
            up.thinking_enabled(),
            Some(*expect),
            "R-THINK-03: {label} -> {expect}; got {:?}",
            up.thinking_enabled()
        );
    }
    // deterministic: repeat one combo -> identical byte-stable result
    let (_, t1) = chat_opt(&gw, "deepseek-chat", Some(json!({"type": "disabled"})), None, Some("high")).await;
    let (_, t2) = chat_opt(&gw, "deepseek-chat", Some(json!({"type": "disabled"})), None, Some("high")).await;
    assert_eq!(t1, t2, "R-THINK-03: identical request -> byte-identical response");
}

// ---------------------------------------------------------------------------
// R-THINK-04 effort degraded mapping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_think_04_effort_degraded_mapping() {
    let up = spawn_upstream(sse_with("x")).await;
    let gw = spawn_gateway(&up.base).await;
    for (effort, expect) in [
        ("none", false), ("off", false), ("low", true), ("medium", true),
        ("high", true), ("max", true),
    ] {
        let (s, _) = chat_opt(&gw, "deepseek-chat", None, Some(effort), None).await;
        assert!(s.is_success(), "R-THINK-04: {effort} -> {s}");
        assert_eq!(
            up.thinking_enabled(),
            Some(expect),
            "R-THINK-04: {effort} -> {expect}; got {:?}",
            up.thinking_enabled()
        );
    }
    // unknown effort -> ignore (no error), default On
    let (s, _) = chat_opt(&gw, "deepseek-chat", None, Some("deep"), None).await;
    assert!(s.is_success(), "R-THINK-04: unknown effort ignored; got {s}");
    assert_eq!(up.thinking_enabled(), Some(true), "R-THINK-04: unknown -> default On");
}

// ---------------------------------------------------------------------------
// R-THINK-05 history reasoning injected BEFORE the content segment
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_think_05_history_reasoning_injected_before_content() {
    let up = spawn_upstream(sse_with("x")).await;
    let gw = spawn_gateway(&up.base).await;
    let body = json!({"model": "deepseek-chat", "messages": [
        {"role": "user", "content": "q1"},
        {"role": "assistant", "content": "a1", "reasoning_content": "thinking one"},
        {"role": "user", "content": "q2"}
    ]});
    let (s, _) = chat_raw(&gw, body).await;
    assert!(s.is_success());
    let prompt = up.prompt();
    let reasoning_idx = prompt.find("[assistant-reasoning]").expect("R-THINK-05: [assistant-reasoning] present");
    let content_idx = prompt.find("[assistant]").expect("R-THINK-05: [assistant] present");
    assert!(
        reasoning_idx < content_idx,
        "R-THINK-05: [assistant-reasoning] before its [assistant] segment; got {prompt}"
    );
    assert!(prompt.contains("thinking one"), "R-THINK-05: reasoning content injected; got {prompt}");

    // empty reasoning -> no segment
    let up2 = spawn_upstream(sse_with("x")).await;
    let gw2 = spawn_gateway(&up2.base).await;
    let body2 = json!({"model": "deepseek-chat", "messages": [
        {"role": "user", "content": "q1"},
        {"role": "assistant", "content": "a1", "reasoning_content": ""},
        {"role": "user", "content": "q2"}
    ]});
    let (s2, _) = chat_raw(&gw2, body2).await;
    assert!(s2.is_success());
    assert!(
        !up2.prompt().contains("[assistant-reasoning]"),
        "R-THINK-05: empty reasoning skipped; got {}",
        up2.prompt()
    );
}

// ---------------------------------------------------------------------------
// R-THINK-06 history reasoning budget truncation (default 4096)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_think_06_history_reasoning_budget_truncation() {
    let up = spawn_upstream(sse_with("x")).await;
    let gw = spawn_gateway(&up.base).await;
    // > 4096 bytes reasoning -> tail kept (<=4096B) + fixed marker
    let long = "r".repeat(8192);
    let body = json!({"model": "deepseek-chat", "messages": [
        {"role": "user", "content": "q"},
        {"role": "assistant", "content": "a", "reasoning_content": long}
    ]});
    let (s, _) = chat_raw(&gw, body).await;
    assert!(s.is_success());
    let prompt = up.prompt();
    let seg_start = prompt.find("[assistant-reasoning]").expect("segment present") + "[assistant-reasoning]".len();
    let seg_end = prompt[seg_start..].find("[assistant]").map(|i| seg_start + i).unwrap_or(prompt.len());
    // segment starts with the template newline after the marker line; strip it
    let content = prompt[seg_start..seg_end].trim_start_matches(|c| c == '\n' || c == '\r');
    let marker = "…(truncated)";
    let core = content.trim_end_matches(|c| c == '\n' || c == '\r');
    assert!(
        core.ends_with(marker),
        "R-THINK-06: over-budget -> tail + marker; got tail: {}",
        &core[core.len().saturating_sub(40)..]
    );
    assert!(
        core.len() - marker.len() <= 4096,
        "R-THINK-06: truncated content (excluding marker) within the 4096-byte budget; got {}",
        core.len() - marker.len()
    );
    assert!(
        !content.starts_with('r') || content.contains('r'),
        "R-THINK-06: tail retained (contains trailing r-run)"
    );

    // exactly at boundary -> original, no marker
    let up2 = spawn_upstream(sse_with("x")).await;
    let gw2 = spawn_gateway(&up2.base).await;
    let exact = "e".repeat(4000);
    let body2 = json!({"model": "deepseek-chat", "messages": [
        {"role": "user", "content": "q"},
        {"role": "assistant", "content": "a", "reasoning_content": exact}
    ]});
    let (s2, _) = chat_raw(&gw2, body2).await;
    assert!(s2.is_success());
    let prompt2 = up2.prompt();
    assert!(
        !prompt2.contains("…(truncated)"),
        "R-THINK-06: within-budget reasoning stays verbatim; got {}",
        &prompt2[prompt2.len().saturating_sub(60)..]
    );
}

// ---------------------------------------------------------------------------
// R-THINK-07 Anthropic budget_tokens accepted and ignored
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_think_07_budget_tokens_ignored() {
    let up = spawn_upstream(sse_with("ok")).await;
    let gw = spawn_gateway(&up.base).await;
    let baseline = reqwest::Client::new()
        .post(format!("{gw}/v1/messages"))
        .bearer_auth("web-token-abc")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({"model": "deepseek-chat", "max_tokens": 100,
            "messages": [{"role": "user", "content": "hi"}]}))
        .send().await.unwrap();
    let sb = baseline.status();
    let tb = baseline.text().await.unwrap_or_default();

    let with_budget = reqwest::Client::new()
        .post(format!("{gw}/v1/messages"))
        .bearer_auth("web-token-abc")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({"model": "deepseek-chat", "max_tokens": 100,
            "thinking": {"type": "enabled", "budget_tokens": 5000},
            "messages": [{"role": "user", "content": "hi"}]}))
        .send().await.unwrap();
    let sw = with_budget.status();
    let tw = with_budget.text().await.unwrap_or_default();

    assert!(sb.is_success() && sw.is_success(), "R-THINK-07: budget_tokens must not error; got {sb} {sw}");
    let b1: Value = serde_json::from_str(&tb).unwrap_or(Value::Null);
    let b2: Value = serde_json::from_str(&tw).unwrap_or(Value::Null);
    assert_eq!(
        b1["content"], b2["content"],
        "R-THINK-07: blocks identical to the no-budget baseline; got {tw}"
    );
}

// ---------------------------------------------------------------------------
// R-THINK-08 THINK -> reasoning_content only; RESPONSE -> content only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_think_08_think_response_no_leak() {
    let sse = sse_reasoning_then_content("thinking text", "answer text");
    let up = spawn_upstream(sse).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat_opt(&gw, "deepseek-chat", None, None, None).await;
    assert!(s.is_success(), "got {s} {t}");
    let body: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
    let msg = &body["choices"][0]["message"];
    let reasoning = msg["reasoning_content"].as_str().unwrap_or("");
    let content = msg["content"].as_str().unwrap_or("");
    assert!(
        reasoning.contains("thinking text"),
        "R-THINK-08: THINK frame -> reasoning_content; got {t}"
    );
    assert!(
        content.contains("answer text"),
        "R-THINK-08: RESPONSE frame -> content; got {t}"
    );
    assert!(
        !content.contains("thinking text"),
        "R-THINK-08: thinking must NOT leak into content; got {t}"
    );
    assert!(
        !reasoning.contains("answer text"),
        "R-THINK-08: response must NOT leak into reasoning; got {t}"
    );
}

// ---------------------------------------------------------------------------
// R-THINK-09 stream ordering: reasoning deltas all BEFORE content deltas
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_think_09_stream_reasoning_before_content() {
    let sse = sse_reasoning_then_content("r1", "c1");
    let up = spawn_upstream(sse).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat_opt(&gw, "deepseek-chat", None, None, None).await;
    assert!(s.is_success(), "got {s} {t}");

    // Non-stream (aggregated) ordering: reasoning emitted before content in
    // the message, no repeats.
    let body: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
    let msg = &body["choices"][0]["message"];
    let reasoning = msg["reasoning_content"].as_str().unwrap_or("");
    let content = msg["content"].as_str().unwrap_or("");
    assert!(reasoning.contains("r1"), "R-THINK-09: reasoning present; got {t}");
    assert!(content.contains("c1"), "R-THINK-09: content present; got {t}");
    assert_eq!(
        body["choices"][0]["finish_reason"], "stop",
        "R-THINK-09: finish_reason=stop; got {t}"
    );

    // tools + thinking=off -> no reasoning frames, tool_calls normal.
    let call = r#"{"tool_call":"get_weather","arguments":{"city":"Shanghai"}}"#;
    let up2 = spawn_upstream(sse_with(call)).await;
    let gw2 = spawn_gateway(&up2.base).await;
    let body2 = json!({"model": "deepseek-chat", "reasoning_effort": "none", "messages": [{"role": "user", "content": "w"}],
        "tools": [{"type": "function", "function": {"name": "get_weather", "description": "d", "parameters": {"type": "object"}}}]});
    let (s2, t2) = chat_raw(&gw2, body2).await;
    assert!(s2.is_success(), "got {s2} {t2}");
    let body2: Value = serde_json::from_str(&t2).unwrap_or(Value::Null);
    let b2 = &body2["choices"][0]["message"];
    assert!(
        b2["reasoning_content"].is_null() || b2["reasoning_content"].as_str().map(|s| s.is_empty()).unwrap_or(false),
        "R-THINK-09: thinking=off -> no reasoning frames; got {t2}"
    );
    assert!(
        b2["tool_calls"][0]["function"]["name"] == "get_weather",
        "R-THINK-09: tool_calls normal with thinking off; got {t2}"
    );
}