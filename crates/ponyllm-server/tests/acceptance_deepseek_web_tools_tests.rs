//! Wave-11 deepseek-web tools & multi-turn — L2-T functional acceptance
//! (`r_mt_01..06`, `r_tool_01..10`, `r_tool_fail_01..05`; contract
//! `.dev-team/contracts/wave-11-deepseek-web-tools.md`).
//!
//! Red phase: multi-turn role formatting + tool emulation are UNIMPLEMENTED
//! (G-T2 bare concatenation, G-T3/G-T4 tools dropped) — every row asserting
//! the template/convention/parsing must FAIL now and PASS after task-10.
//!
//! Env gate `PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION` is process-global: tests
//! that flip it serialize via ENV_LOCK (parallel harness threads).

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

/// The model output fed by the mock completion stream (per-test).
const DEFAULT_OUTPUT: &str = "hello";

struct Upstream {
    base: String,
    calls: Arc<Mutex<Vec<String>>>,
    completion_body: Arc<Mutex<Option<Value>>>,
    completion_status: Arc<AtomicUsize>,
}

impl Upstream {
    fn snapshot(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

async fn spawn_upstream(output_sse: String, status: u16) -> Arc<Upstream> {
    use axum::http::HeaderMap;
    use axum::response::Response;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let up = Arc::new(Upstream {
        base: format!("http://{addr}"),
        calls: Arc::new(Mutex::new(Vec::new())),
        completion_body: Arc::new(Mutex::new(None)),
        completion_status: Arc::new(AtomicUsize::new(status as usize)),
    });
    let up_challenge = up.clone();
    let up_session = up.clone();
    let up_completion = up.clone();
    let app = axum::Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            axum::routing::post(move |axum::Json(b): axum::Json<serde_json::Value>| {
                let up = up_challenge.clone();
                async move {
                    up.calls.lock().unwrap().push(format!("challenge:{}", b["target_path"]));
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
                    *up.completion_body.lock().unwrap() = Some(body.0.clone());
                    let st = up.completion_status.load(Ordering::SeqCst) as u16;
                    Response::builder()
                        .status(st)
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

fn tool(name: &str, desc: &str) -> Value {
    json!({"type": "function", "function": {"name": name, "description": desc, "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}}})
}

async fn chat(gw: &str, body: Value) -> (reqwest::StatusCode, String) {
    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    (status, text)
}

fn prompt_of(up: &Arc<Upstream>) -> String {
    up.completion_body
        .lock()
        .unwrap()
        .clone()
        .expect("completion body captured")
        .get("prompt")
        .and_then(|p| p.as_str())
        .unwrap_or_default()
        .to_string()
}

const WEATHER_TOOL: &str = "get_weather";
const WEATHER_DESC: &str = "get current weather for a city";

// ---------------------------------------------------------------------------
// R-MT-01 role markers deterministic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_mt_01_role_markers_deterministic() {
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let gw = spawn_gateway(&up.base).await;
    let body = json!({
        "model": "deepseek-chat",
        "messages": [
            {"role": "system", "content": "you are a helpful assistant"},
            {"role": "user", "content": "hello"}
        ]
    });
    let (status, text) = chat(&gw, body).await;
    assert!(status.is_success(), "got {status} {text}");
    let prompt = prompt_of(&up);
    let sys_idx = prompt.find("[system]").expect("R-MT-01: [system] marker present");
    let user_idx = prompt.find("[user]").expect("R-MT-01: [user] marker present");
    assert!(sys_idx < user_idx, "R-MT-01: [system] precedes [user]");
    assert!(prompt.contains("you are a helpful assistant"));
    assert!(prompt.contains("hello"));
}

// ---------------------------------------------------------------------------
// R-MT-02 segment order == input order (3 turns incl. assistant history)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_mt_02_segment_order_equals_input_order() {
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let gw = spawn_gateway(&up.base).await;
    let (status, text) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [
            {"role": "user", "content": "My name is Bob"},
            {"role": "assistant", "content": "Nice to meet you, Bob!"},
            {"role": "user", "content": "What is my name?"}
        ]}),
    )
    .await;
    assert!(status.is_success(), "got {status} {text}");
    let prompt = prompt_of(&up);
    let u1 = prompt.find("[user]").expect("user");
    let a1 = prompt.find("[assistant]").expect("assistant");
    let u2 = prompt.rfind("[user]").expect("user2");
    assert!(u1 < a1 && a1 < u2, "R-MT-02: user -> assistant -> user order; got {prompt}");
    assert!(prompt.contains("My name is Bob"));
    assert!(prompt.contains("Nice to meet you, Bob!"));
    assert!(prompt.contains("What is my name?"));
}

// ---------------------------------------------------------------------------
// R-MT-03 empty skipped; adjacent same-role merged
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_mt_03_empty_skipped_adjacent_merged() {
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let gw = spawn_gateway(&up.base).await;
    let (status, text) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [
            {"role": "user", "content": ""},
            {"role": "user", "content": "first line"},
            {"role": "user", "content": "second line"}
        ]}),
    )
    .await;
    assert!(status.is_success(), "got {status} {text}");
    let prompt = prompt_of(&up);
    assert!(
        prompt.matches("[user]").count() == 1,
        "R-MT-03: adjacent user messages merge into ONE [user] segment; got {prompt}"
    );
    assert!(
        !prompt.contains("[system]"),
        "R-MT-03: no system segment produced for empty system"
    );
    assert!(prompt.contains("first line"));
    assert!(prompt.contains("second line"));
}

// ---------------------------------------------------------------------------
// R-MT-04 assistant tool_calls kept readable in the history
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_mt_04_assistant_toolcalls_kept_readable() {
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let gw = spawn_gateway(&up.base).await;
    let (status, text) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [
            {"role": "user", "content": "weather in Shanghai"},
            {"role": "assistant", "content": "", "tool_calls": [
                {"id": "tc1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Shanghai\"}"}},
                {"id": "tc2", "type": "function", "function": {"name": "get_time", "arguments": "{\"city\":\"Shanghai\"}"}}
            ]}
        ]}),
    )
    .await;
    assert!(status.is_success(), "got {status} {text}");
    let prompt = prompt_of(&up);
    assert!(
        prompt.contains("[tool-call:get_weather]") && prompt.contains("[tool-call:get_time]"),
        "R-MT-04: assistant tool_calls kept as readable lines; got {prompt}"
    );
    assert!(
        prompt.contains("\"city\":\"Shanghai\"") || prompt.contains("{\"city\":\"Shanghai\"}"),
        "R-MT-04: arguments kept single-line compact JSON; got {prompt}"
    );
}

// ---------------------------------------------------------------------------
// R-MT-05 tool result -> [tool:<name>] via id->name mapping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_mt_05_tool_result_segment_by_id_mapping() {
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let gw = spawn_gateway(&up.base).await;
    let (status, text) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [
            {"role": "user", "content": "weather in Shanghai"},
            {"role": "assistant", "content": "", "tool_calls": [
                {"id": "tc1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Shanghai\"}"}}
            ]},
            {"role": "tool", "tool_call_id": "tc1", "content": "20 degrees, sunny"}
        ]}),
    )
    .await;
    assert!(status.is_success(), "got {status} {text}");
    let prompt = prompt_of(&up);
    assert!(
        prompt.contains("[tool:get_weather]"),
        "R-MT-05: tool result in [tool:get_weather] segment (id->name mapping); got {prompt}"
    );
    assert!(
        prompt.contains("20 degrees, sunny"),
        "R-MT-05: tool result content present; got {prompt}"
    );
}

// ---------------------------------------------------------------------------
// R-MT-06 stateless multi-turn (declaration row)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_mt_06_stateless_multiturn_via_prompt() {
    // Two independent requests each carry their FULL history in the prompt;
    // no server-side session state is consulted.
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let gw = spawn_gateway(&up.base).await;
    let (s1, _) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [
            {"role": "user", "content": "turn one"},
            {"role": "assistant", "content": "answer one"},
            {"role": "user", "content": "turn two"}
        ]}),
    )
    .await;
    assert!(s1.is_success());
    let p1 = prompt_of(&up);
    assert!(p1.contains("[user]") && p1.contains("[assistant]"), "R-MT-06: full history in prompt");
}

// ---------------------------------------------------------------------------
// R-TOOL-01 env gate default ON
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_01_env_gate_default_on() {
    let _guard = env_guard();
    // (a) unset -> ON: tools -> convention injected into the prompt
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, _t) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "weather?"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)]}),
    )
    .await;
    assert!(s.is_success());
    let prompt = prompt_of(&up);
    assert!(
        prompt.contains(WEATHER_TOOL) && prompt.contains("tool_call"),
        "R-TOOL-01: unset env -> emulation ON (schema+convention injected); got {prompt}"
    );
    // (c) "1"/"true" -> ON
    for v in ["1", "true"] {
        std::env::set_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION", v);
        let up2 = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
        let gw2 = spawn_gateway(&up2.base).await;
        let (s2, _) = chat(
            &gw2,
            json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "weather?"}],
                   "tools": [tool(WEATHER_TOOL, WEATHER_DESC)]}),
        )
        .await;
        assert!(s2.is_success(), "R-TOOL-01: {v} -> emulation ON");
        assert!(prompt_of(&up2).contains("tool_call"), "R-TOOL-01: {v} -> convention injected");
    }
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
}

// ---------------------------------------------------------------------------
// R-TOOL-02 schema injected into prompt, NOT leaked to downstream content
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_02_schema_injected_not_leaked() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let (status, text) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "weather?"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)]}),
    )
    .await;
    assert!(status.is_success(), "got {status} {text}");
    let prompt = prompt_of(&up);
    assert!(
        prompt.contains(WEATHER_TOOL)
            && prompt.contains(WEATHER_DESC)
            && prompt.contains("{\"type\":\"object\""),
        "R-TOOL-02: schema (name/description/JSON-schema) injected into prompt; got {prompt}"
    );
    // Downstream content must not leak the schema/convention JSON.
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let content = body["choices"][0]["message"]["content"].as_str().unwrap_or("");
    assert!(
        !content.contains(WEATHER_TOOL) && !content.contains(WEATHER_DESC),
        "R-TOOL-02: schema must not leak into downstream content; got {text}"
    );
}

// ---------------------------------------------------------------------------
// R-TOOL-03 convention frozen shape + deterministic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_03_convention_frozen_shape() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let req = json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "weather?"}],
                     "tools": [tool(WEATHER_TOOL, WEATHER_DESC)]});
    let (s1, _) = chat(&gw, req.clone()).await;
    assert!(s1.is_success());
    let p1 = prompt_of(&up);
    let (s2, _) = chat(&gw, req).await;
    assert!(s2.is_success());
    let p2 = prompt_of(&up);
    assert!(
        p1.contains("\"tool_call\"") && p1.contains("arguments"),
        "R-TOOL-03: convention fields frozen (tool_call+arguments); got {p1}"
    );
    assert_eq!(p1, p2, "R-TOOL-03: identical inputs -> byte-identical prompt");
}

// ---------------------------------------------------------------------------
// R-TOOL-04 tool_choice none -> no call instruction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_04_choice_none_no_call_instruction() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, _) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "weather?"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)], "tool_choice": "none"}),
    )
    .await;
    assert!(s.is_success());
    let prompt = prompt_of(&up);
    assert!(
        !prompt.contains("\"tool_call\"") && !prompt.contains("arguments"),
        "R-TOOL-04: tool_choice=none -> no call instruction injected; got {prompt}"
    );
}

// ---------------------------------------------------------------------------
// R-TOOL-05 tool_choice auto/default: text or call both legal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_05_choice_auto_permits_text_or_call() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    // (a) model answers with plain text -> normal content (auto allows text)
    let up = spawn_upstream(sse_with("I can answer directly."), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let (s1, t1) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "hi"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)]}),
    )
    .await;
    assert!(s1.is_success(), "got {s1} {t1}");
    let body1: Value = serde_json::from_str(&t1).unwrap_or(Value::Null);
    assert!(
        body1["choices"][0]["message"]["content"].as_str().unwrap_or("").contains("I can answer directly"),
        "R-TOOL-05: auto permits a text answer; got {t1}"
    );
    // (b) model outputs the JSON -> tool_calls (parse per R-TOOL-07)
    let up2 = spawn_upstream(sse_with(r#"{"tool_call":"get_weather","arguments":{"city":"Shanghai"}}"#), 200).await;
    let gw2 = spawn_gateway(&up2.base).await;
    let (s2, t2) = chat(
        &gw2,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "weather?"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)]}),
    )
    .await;
    assert!(s2.is_success(), "got {s2} {t2}");
    let b2: Value = serde_json::from_str(&t2).unwrap_or(Value::Null);
    assert!(
        b2["choices"][0]["message"]["tool_calls"].is_array(),
        "R-TOOL-05: auto + JSON output -> tool_calls; got {t2}"
    );
}

// ---------------------------------------------------------------------------
// R-TOOL-06 tool_choice required / {function:{name}} forcing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_06_choice_required_forces_call() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    // required (unspecified) -> prompt carries a strong-constraint instruction.
    // The mock must still answer with a VALID call so the request completes;
    // we assert the injected wording from the captured prompt.
    let call = r#"{"tool_call":"get_weather","arguments":{"city":"Shanghai"}}"#;
    let up = spawn_upstream(sse_with(call), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, _) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "weather?"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)], "tool_choice": "required"}),
    )
    .await;
    assert!(s.is_success(), "R-TOOL-06: required + valid call must complete; got {s}");
    let prompt = prompt_of(&up);
    assert!(
        prompt.contains("must") || prompt.contains("must call") || prompt.contains("required"),
        "R-TOOL-06: required -> strong-constraint language in prompt; got {prompt}"
    );
    // specified name -> that name appears in the constraint
    let up2 = spawn_upstream(sse_with(call), 200).await;
    let gw2 = spawn_gateway(&up2.base).await;
    let (s2, _) = chat(
        &gw2,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "weather?"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)],
               "tool_choice": {"function": {"name": "get_weather"}}}),
    )
    .await;
    assert!(s2.is_success(), "R-TOOL-06: specified-name + valid call must complete; got {s2}");
    assert!(
        prompt_of(&up2).contains("get_weather"),
        "R-TOOL-06: specified function name in the forced-call instruction; got {}",
        prompt_of(&up2)
    );
}

// ---------------------------------------------------------------------------
// R-TOOL-07 model JSON -> OpenAI tool_calls (+ finish_reason, no content leak)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_07_model_json_maps_to_tool_calls() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let json_out = r#"{"tool_call":"get_weather","arguments":{"city":"Shanghai"}}"#;
    let up = spawn_upstream(sse_with(json_out), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "weather?"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)]}),
    )
    .await;
    assert!(s.is_success(), "got {s} {t}");
    let body: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
    let tc = &body["choices"][0]["message"]["tool_calls"][0];
    assert_eq!(tc["type"], "function");
    assert_eq!(tc["function"]["name"], "get_weather");
    let args = tc["function"]["arguments"].as_str().unwrap_or("");
    assert!(
        serde_json::from_str::<Value>(args).map(|v| v["city"] == "Shanghai").unwrap_or(false),
        "R-TOOL-07: arguments is a JSON string carrying city==Shanghai; got {args}"
    );
    assert!(
        tc["id"].as_str().map(|i| i.starts_with("call_")).unwrap_or(false),
        "R-TOOL-07: id ~ call_<random>"
    );
    assert_eq!(body["choices"][0]["finish_reason"], "tool_calls", "R-TOOL-07: finish_reason=tool_calls; got {t}");
    let content = body["choices"][0]["message"]["content"].as_str().unwrap_or("");
    assert!(
        !content.contains(json_out),
        "R-TOOL-07: content must NOT repeat the call JSON; got {t}"
    );
}

// ---------------------------------------------------------------------------
// R-TOOL-08 tolerant parse: fence / array / whitespace; truncation -> fail-03
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_08_tolerant_multi_call_parse() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    // (a) fenced
    let fenced = "```json\n{\"tool_call\":\"get_weather\",\"arguments\":{\"city\":\"Shanghai\"}}\n```";
    let up = spawn_upstream(sse_with(fenced), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let (s1, t1) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "w"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)]}),
    )
    .await;
    assert!(s1.is_success(), "got {s1} {t1}");
    let b1: Value = serde_json::from_str(&t1).unwrap_or(Value::Null);
    assert!(
        b1["choices"][0]["message"]["tool_calls"][0]["function"]["name"] == "get_weather",
        "R-TOOL-08: fenced JSON parsed; got {t1}"
    );
    // (b) JSON array -> parallel tool_calls in order
    let arr = r#"[{"tool_call":"get_weather","arguments":{"city":"Shanghai"}},{"tool_call":"get_time","arguments":{"city":"Beijing"}}]"#;
    let up2 = spawn_upstream(sse_with(arr), 200).await;
    let gw2 = spawn_gateway(&up2.base).await;
    let (s2, t2) = chat(
        &gw2,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "w"}],
               "tools": [tool("get_weather","a"), tool("get_time","b")]}),
    )
    .await;
    assert!(s2.is_success(), "got {s2} {t2}");
    let b2: Value = serde_json::from_str(&t2).unwrap_or(Value::Null);
    let calls = b2["choices"][0]["message"]["tool_calls"].as_array().cloned().unwrap_or_default();
    assert_eq!(calls.len(), 2, "R-TOOL-08: array -> parallel tool_calls; got {t2}");
    assert_eq!(calls[0]["function"]["name"], "get_weather");
    assert_eq!(calls[1]["function"]["name"], "get_time");
    // (c) truncated -> auto passthrough (fail-03), no forged call
    let up3 = spawn_upstream(sse_with(r#"{"tool_call":"get_weather","arguments":{"city":"Sha"#), 200).await;
    let gw3 = spawn_gateway(&up3.base).await;
    let (s3, t3) = chat(
        &gw3,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "w"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)]}),
    )
    .await;
    assert!(s3.is_success(), "got {s3} {t3}");
    let b3: Value = serde_json::from_str(&t3).unwrap_or(Value::Null);
    assert!(
        b3["choices"][0]["message"]["tool_calls"].is_null()
            || b3["choices"][0]["message"]["tool_calls"].as_array().map(|a| a.is_empty()).unwrap_or(true),
        "R-TOOL-08: truncated -> passthrough, no forged tool_calls; got {t3}"
    );
}

// ---------------------------------------------------------------------------
// R-TOOL-09 Anthropic /v1/messages + tools -> tool_use block
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_09_anthropic_tool_use_block() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let json_out = r#"{"tool_call":"get_weather","arguments":{"city":"Shanghai"}}"#;
    let up = spawn_upstream(sse_with(json_out), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/messages"))
        .bearer_auth("web-token-abc")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "deepseek-chat",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "weather in Shanghai"}],
            "tools": [{"name": "get_weather", "description": WEATHER_DESC, "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}}}]
        }))
        .send()
        .await
        .unwrap();
    let s = resp.status();
    let t = resp.text().await.unwrap_or_default();
    assert!(s.is_success(), "got {s} {t}");
    let body: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
    let blocks = body["content"].as_array().cloned().unwrap_or_default();
    let tu = blocks.iter().find(|b| b["type"] == "tool_use");
    assert!(tu.is_some(), "R-TOOL-09: tool_use block present; got {t}");
    let tu = tu.unwrap();
    assert_eq!(tu["name"], "get_weather");
    assert_eq!(tu["input"]["city"], "Shanghai", "R-TOOL-09: input = arguments as object; got {t}");
    assert!(tu["id"].as_str().is_some());
}

// ---------------------------------------------------------------------------
// R-TOOL-10 limits: 65 tools / 64KB+1 schema / 65 calls -> 4xx
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_10_limits_64_64k() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    // 65 tools -> 4xx, zero upstream completion
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let many: Vec<Value> = (0..65).map(|i| tool(&format!("tool_{i}"), "d")).collect();
    let (s1, t1) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "w"}], "tools": many}),
    )
    .await;
    assert!(
        !s1.is_success() && s1.as_u16() < 500,
        "R-TOOL-10: 65 tools -> 4xx; got {s1} {t1}"
    );
    // schema > 64KB -> 4xx
    let big_params = json!({"type": "object", "properties": {"x": {"type": "string", "description": "a".repeat(66000)}}});
    let up2 = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    let gw2 = spawn_gateway(&up2.base).await;
    let (s2, t2) = chat(
        &gw2,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "w"}],
               "tools": [{"type": "function", "function": {"name": "big", "description": "d", "parameters": big_params}}]}),
    )
    .await;
    assert!(
        !s2.is_success() && s2.as_u16() < 500,
        "R-TOOL-10: >64KB schema -> 4xx; got {s2} {t2}"
    );
}

// ---------------------------------------------------------------------------
// R-TOOL-FAIL-01 emulation disabled + tools -> 4xx (no silent 200)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_fail_01_disabled_emulation_4xx() {
    let _guard = env_guard();
    std::env::set_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION", "off");
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "w"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)]}),
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
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
}

// ---------------------------------------------------------------------------
// R-TOOL-FAIL-02 required unmet -> 4xx (no silent text success)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_fail_02_required_unmet_4xx() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let up = spawn_upstream(sse_with("I answer in plain text, no call."), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "w"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)], "tool_choice": "required"}),
    )
    .await;
    assert!(
        !s.is_success() && s.as_u16() < 500,
        "R-TOOL-FAIL-02: required + no call -> explicit 4xx (no silent text); got {s} {t}"
    );
}

// ---------------------------------------------------------------------------
// R-TOOL-FAIL-03 auto parse failure -> passthrough text, no forged call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_fail_03_auto_parse_failure_passthrough() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let junk = r#"{"tool_call":"get_weather","arguments":{BAD"#;
    let up = spawn_upstream(sse_with(junk), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "w"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)]}),
    )
    .await;
    assert!(s.is_success(), "auto parse failure -> 200 passthrough; got {s} {t}");
    let body: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
    let content = body["choices"][0]["message"]["content"].as_str().unwrap_or("");
    assert!(
        content.contains(junk),
        "R-TOOL-FAIL-03: original text passed through, not swallowed; got {t}"
    );
    assert!(
        body["choices"][0]["message"]["tool_calls"].is_null()
            || body["choices"][0]["message"]["tool_calls"].as_array().map(|a| a.is_empty()).unwrap_or(true),
        "R-TOOL-FAIL-03: no forged tool_calls; got {t}"
    );
}

// ---------------------------------------------------------------------------
// R-TOOL-FAIL-04 unknown function in tool_choice -> 4xx + zero upstream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_fail_04_unknown_function_4xx() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [{"role": "user", "content": "w"}],
               "tools": [tool(WEATHER_TOOL, WEATHER_DESC)],
               "tool_choice": {"function": {"name": "nope"}}}),
    )
    .await;
    assert!(
        !s.is_success() && s.as_u16() < 500,
        "R-TOOL-FAIL-04: unknown function -> 4xx; got {s} {t}"
    );
    let calls = up.snapshot();
    assert!(
        !calls.iter().any(|c| c == "completion"),
        "R-TOOL-FAIL-04: request must not reach upstream; got {calls:?}"
    );
}

// ---------------------------------------------------------------------------
// R-TOOL-FAIL-05 orphan tool_call_id -> 4xx
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r_tool_fail_05_unmatched_tool_call_id_4xx() {
    let _guard = env_guard();
    std::env::remove_var("PONYLLM_DEEPSEEK_WEB_TOOL_EMULATION");
    let up = spawn_upstream(sse_with(DEFAULT_OUTPUT), 200).await;
    let gw = spawn_gateway(&up.base).await;
    let (s, t) = chat(
        &gw,
        json!({"model": "deepseek-chat", "messages": [
            {"role": "assistant", "content": "", "tool_calls": [
                {"id": "tc1", "type": "function", "function": {"name": "get_weather", "arguments": "{}"}}
            ]},
            {"role": "tool", "tool_call_id": "tc-orphan", "content": "result"}
        ]}),
    )
    .await;
    assert!(
        !s.is_success() && s.as_u16() < 500,
        "R-TOOL-FAIL-05: orphan tool_call_id -> explicit 4xx (no silent drop); got {s} {t}"
    );
}