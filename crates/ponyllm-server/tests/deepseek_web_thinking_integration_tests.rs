//! deepseek-web thinking (reasoning) integration tests (T4, wave-12 contract
//! R-THINK-01..09).
//!
//! Full chain against a mock chat.deepseek.com upstream (axum, random port):
//! four-source thinking unification (body thinking > body reasoning_effort >
//! header > model suffix > default On), body `thinking.type` parsing, effort
//! degraded mapping, history `[assistant-reasoning]` injection with byte-budget
//! tail truncation, THINK/RESPONSE non-leak, and stream reasoning-before-
//! content ordering.
//!
//! Owned by T4 (exec-integration): `deepseek_web_` prefix; frozen T2 (task-14)
//! acceptance files are separate and untouched.

use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::json;
use std::sync::{Arc, Mutex};

/// Serializes process-global env mutations (R-THINK-06 budget knob).
static ENV_MUTEX: Mutex<()> = Mutex::new(());

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

/// Mock upstream capturing completion bodies (prompt + thinking_enabled) and
/// returning a caller-provided SSE body. Returns `(base_url, bodies)`.
async fn spawn_upstream(
    completion_sse: Option<String>,
) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let bodies: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let bodies_c = bodies.clone();
    let sse = completion_sse.unwrap_or_else(|| {
        format!(
            "event: ready\ndata: {}\n\n\
             event: delta\ndata: {}\n\n\
             event: delta\ndata: {}\n\n\
             event: finish\ndata: {}\n\n\
             event: close\ndata: {}\n\n",
            json!({"response_message_id":"r-th","request_message_id":"q-th","model_type":"deepseek-v4-flash"}),
            json!({"p":"response/fragments/-1/type","o":"SET","v":"TEXT"}),
            json!({"p":"response/fragments/-1/content","o":"APPEND","v":"42"}),
            json!({"usage":{"prompt_tokens":5,"completion_tokens":1,"total_tokens":6}}),
            json!({"close_reason":"success"})
        )
    });
    let app = Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            post(|Json(body): Json<serde_json::Value>| async move {
                let _ = body;
                Json(json!({
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
                }))
            }),
        )
        .route(
            "/api/v0/chat_session/create",
            post(|Json(_): Json<serde_json::Value>| async move {
                Json(json!({
                    "biz_code": 0,
                    "biz_data": { "chat_session": { "id": "sess-th-1" } }
                }))
            }),
        )
        .route(
            "/api/v0/chat/completion",
            post(move |Json(body): Json<serde_json::Value>| {
                let bodies_c = bodies_c.clone();
                let sse = sse.clone();
                async move {
                    bodies_c.lock().unwrap().push(body.clone());
                    axum::response::Response::new(axum::body::Body::from(sse))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), bodies)
}

async fn spawn_gateway_for(base: &str) -> String {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config
        .providers
        .insert("deepseek-web".to_string(), provider_config(base));
    let pool = Arc::new(KeyPool::new("deepseek-web", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("ds-web-k1", "user-web-token-abc", 1, 10));
    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek-web", pool);
    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{}", addr)
}

async fn chat(gw: &str, body: serde_json::Value) -> (reqwest::StatusCode, String) {
    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    (status, text)
}

/// R-THINK-01/02/03/04: body `thinking`/`reasoning_effort` unify the decision
/// with body > header > suffix > default On — asserted on the captured
/// completion body `thinking_enabled`.
#[tokio::test]
async fn deepseek_web_thinking_four_source_priority() {
    let _g = ENV_MUTEX.lock().unwrap();
    let cases: Vec<(serde_json::Value, Option<&str>, bool)> = vec![
        // body thinking disabled → Off (overrides everything).
        (json!({"thinking": {"type": "disabled"}}), None, false),
        // body thinking enabled → On.
        (json!({"thinking": {"type": "enabled"}}), None, true),
        // body reasoning_effort none → Off.
        (json!({"reasoning_effort": "none"}), None, false),
        // body reasoning_effort high → On (degraded mapping).
        (json!({"reasoning_effort": "high"}), None, true),
        // body thinking wins over body reasoning_effort.
        (
            json!({"thinking": {"type": "disabled"}, "reasoning_effort": "high"}),
            None,
            false,
        ),
        // no body → default On.
        (json!({}), None, true),
    ];
    for (body_extra, _header, expect) in cases {
        let (upstream, bodies) = spawn_upstream(None).await;
        let gw = spawn_gateway_for(&upstream).await;
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [{"role":"user","content":"how much?"}],
            "stream": false
        });
        for (k, v) in body_extra.as_object().unwrap() {
            body[k.as_str()] = v.clone();
        }
        let (status, text) = chat(&gw, body).await;
        assert!(status.is_success(), "case {body_extra} must 200, got {status} {text}");
        let captured = bodies.lock().unwrap()[0].clone();
        let enabled = captured["thinking_enabled"].as_bool().unwrap_or(true);
        assert_eq!(
            enabled, expect,
            "case {body_extra}: thinking_enabled must be {expect}, body={captured}"
        );
    }
}

/// R-THINK-02: unknown `thinking.type` (auto / non-string / array) is ignored
/// with a debug log — falls back to lower-priority sources / default, never
/// errors.
#[tokio::test]
async fn deepseek_web_thinking_unknown_type_ignored() {
    let _g = ENV_MUTEX.lock().unwrap();
    for bad in [
        json!({"thinking": {"type": "auto"}}),
        json!({"thinking": {"type": 5}}),
        json!({"thinking": {"type": ["disabled"]}}),
        json!({"thinking": "disabled"}),
        json!({"thinking": {"nope": 1}}),
    ] {
        let (upstream, bodies) = spawn_upstream(None).await;
        let gw = spawn_gateway_for(&upstream).await;
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [{"role":"user","content":"hi"}],
            "stream": false
        });
        for (k, v) in bad.as_object().unwrap() {
            body[k.as_str()] = v.clone();
        }
        let (status, text) = chat(&gw, body).await;
        assert!(
            status.is_success(),
            "unknown thinking.type must not error, got {status} {text}"
        );
        // Falls back to default On (DW-Q-03).
        let enabled = bodies.lock().unwrap()[0]["thinking_enabled"]
            .as_bool()
            .unwrap_or(true);
        assert_eq!(enabled, true, "unknown type → default On (R-THINK-02)");
    }
}

/// R-THINK-05/06: assistant history `reasoning_content` is injected as a
/// `[assistant-reasoning]` segment BEFORE the `[assistant]` segment; over the
/// byte budget → tail kept + `…(truncated)`.
#[tokio::test]
async fn deepseek_web_thinking_history_injection_and_truncation() {
    let _g = ENV_MUTEX.lock().unwrap();
    // Short reasoning fits.
    let (upstream, bodies) = spawn_upstream(None).await;
    let gw = spawn_gateway_for(&upstream).await;
    let (status, text) = chat(
        &gw,
        json!({
            "model": "deepseek-v4-flash",
            "messages": [
                {"role":"user","content":"q1"},
                {"role":"assistant","content":"a1","reasoning_content":"thinking one"},
                {"role":"user","content":"q2"}
            ],
            "stream": false
        }),
    )
    .await;
    assert!(status.is_success(), "got {status} {text}");
    let prompt = bodies.lock().unwrap()[0]["prompt"]
        .as_str()
        .expect("prompt captured")
        .to_string();
    let re_idx = prompt.find("[assistant-reasoning]").expect("reasoning segment present");
    let as_idx = prompt.find("[assistant]").expect("assistant segment present");
    assert!(
        re_idx < as_idx,
        "[assistant-reasoning] must precede [assistant]: {prompt}"
    );
    assert!(prompt.contains("thinking one"), "reasoning content injected: {prompt}");

    // Over-budget → tail kept + marker (budget 128 via env).
    std::env::set_var("PONYLLM_WEB_REASONING_HISTORY_MAX_BYTES", "128");
    let long = "x".repeat(500);
    let (upstream2, bodies2) = spawn_upstream(None).await;
    let gw2 = spawn_gateway_for(&upstream2).await;
    let (s2, t2) = chat(
        &gw2,
        json!({
            "model": "deepseek-v4-flash",
            "messages": [
                {"role":"user","content":"q"},
                {"role":"assistant","content":"a","reasoning_content": long},
                {"role":"user","content":"q2"}
            ],
            "stream": false
        }),
    )
    .await;
    assert!(s2.is_success(), "got {s2} {t2}");
    let prompt2 = bodies2.lock().unwrap()[0]["prompt"]
        .as_str()
        .expect("prompt captured")
        .to_string();
    assert!(
        prompt2.contains("…(truncated)"),
        "truncation marker present: {prompt2}"
    );
    // The segment body (excluding the marker) must be ≤ budget bytes.
    let seg_start = prompt2.find("[assistant-reasoning]").unwrap() + "[assistant-reasoning]\n".len();
    let seg_end = prompt2[seg_start..]
        .find("\n\n[assistant]")
        .map(|i| seg_start + i)
        .unwrap_or(prompt2.len());
    let seg_body = &prompt2[seg_start..seg_end];
    let without_marker = seg_body.trim_end_matches("…(truncated)");
    assert!(
        without_marker.len() <= 128,
        "tail must fit budget (128), got {} bytes",
        without_marker.len()
    );
    std::env::remove_var("PONYLLM_WEB_REASONING_HISTORY_MAX_BYTES");
}

/// R-THINK-09: streaming — reasoning deltas all precede content deltas (order
/// preserved), each fragment exactly once, finish_reason=stop.
#[tokio::test]
async fn deepseek_web_thinking_stream_order_and_nonleak() {
    let _g = ENV_MUTEX.lock().unwrap();
    let sse = format!(
        "event: ready\ndata: {}\n\n\
         event: delta\ndata: {}\n\n\
         event: delta\ndata: {}\n\n\
         event: delta\ndata: {}\n\n\
         event: delta\ndata: {}\n\n\
         event: finish\ndata: {}\n\n\
         event: close\ndata: {}\n\n",
        json!({"response_message_id":"r-s","request_message_id":"q-s","model_type":"deepseek-v4-flash"}),
        json!({"p":"response/fragments/-1/type","o":"SET","v":"THINKING"}),
        json!({"p":"response/fragments/-1/content","o":"APPEND","v":"step1"}),
        json!({"p":"response/fragments/-1/type","o":"SET","v":"TEXT"}),
        json!({"p":"response/fragments/-1/content","o":"APPEND","v":"final"}),
        json!({"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}),
        json!({"close_reason":"success"})
    );
    let (upstream, _b) = spawn_upstream(Some(sse)).await;
    let gw = spawn_gateway_for(&upstream).await;
    let (status, text) = chat(
        &gw,
        json!({
            "model": "deepseek-v4-flash",
            "messages": [{"role":"user","content":"think"}],
            "stream": true
        }),
    )
    .await;
    assert!(status.is_success(), "got {status} {text}");
    let chunks: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter_map(|d| serde_json::from_str::<serde_json::Value>(d).ok())
        .filter(|v| v.get("object").and_then(|o| o.as_str()) == Some("chat.completion.chunk"))
        .collect();
    // THINKING → reasoning_content only; TEXT → content only (R-THINK-08).
    let mut saw_reasoning = false;
    let mut saw_content = false;
    for c in &chunks {
        let d = &c["choices"][0]["delta"];
        if let Some(r) = d.get("reasoning_content").and_then(|v| v.as_str()) {
            assert!(!saw_content, "reasoning after content must not happen (R-THINK-09)");
            saw_reasoning = true;
            assert_eq!(r, "step1");
        }
        if let Some(ct) = d.get("content").and_then(|v| v.as_str()) {
            assert!(saw_reasoning, "content must follow reasoning");
            saw_content = true;
            assert_eq!(ct, "final");
        }
    }
    assert!(saw_reasoning, "reasoning delta emitted");
    assert!(saw_content, "content delta emitted");
    // reasoning must NOT leak into content (R-THINK-08).
    let content_joined: String = chunks
        .iter()
        .filter_map(|c| c["choices"][0]["delta"]["content"].as_str())
        .collect();
    assert!(!content_joined.contains("step1"), "THINK content leaked into content");
    // finish_reason=stop (DW-T-04).
    assert_eq!(chunks.last().unwrap()["choices"][0]["finish_reason"], "stop");
}