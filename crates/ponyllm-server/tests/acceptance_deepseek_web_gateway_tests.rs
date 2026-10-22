//! Wave-9 deepseek-web — L2-T gateway acceptance: SSE -> OpenAI chunk
//! translation over the real HTTP full chain (`dw_t_01..09`, contract §5) +
//! gateway-level error propagation extras (`gw_*`).
//!
//! Harness: axum mock upstream on a random port implementing the three
//! deepseek endpoints (create_pow_challenge / chat_session/create /
//! chat/completion SSE) per REA §1-§5; the real gateway (`create_app`) with a
//! `deepseek-web` provider pointing at the mock — the request_routing_tests
//! pattern (TCP listener :0 + axum::serve).
//!
//! COMPILE DEPENDENCY (red-phase evidence = compile-blocked until task-4):
//! the provider is declared with `UpstreamProtocol::DeepSeekWeb`, the
//! deepseek-web config form implemented by executor task-4 (task card §(a)
//! "UpstreamProtocol 扩展"). This file is the frozen acceptance surface —
//! implementers must not modify it; the symbol must exist under that name.
//! The DW-T-* semantics are additionally pinned at the core level by the
//! frozen H-09 parser tests (deepseek_web_sse_tests.rs), which are
//! runtime-red on the stubs at this phase.

use ponyllm_core::pool::{ApiKeyEntry, BillingMode, KeyPool, RoutingStrategy, UpstreamProtocol};
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const CHALLENGE_OK: &str = r#"{"data":{"biz_code":0,"biz_data":{"challenge":{
"algorithm":"DeepSeekHashV1",
"challenge":"98ba09594cfd94513beeccb58fc56795f2a95e7285aafb58792bd8a2a69d329a",
"salt":"s000",
"difficulty":8,
"signature":"sig2",
"expireAt":1729666800000
}},"biz_msg":"ok"}}"#;

const SESSION_OK: &str = r#"{"data":{"biz_code":0,"biz_data":{"chat_session":{"id":"sess-mock-1","title":"New chat"}},"biz_msg":"ok"}}"#;

/// Full-chain SSE fixture exercising DW-T-01..09 (except the degradation and
/// close-reason scenarios, which use dedicated fixtures).
const T_STREAM: &str = "event: ready\ndata: {\"response_message_id\":\"rsp_01\",\"request_message_id\":\"req_01\",\"model_type\":\"deepseek-chat\"}\n\n\
event: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/content/0/type\",\"v\":\"text\"}\n\n\
event: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/content/0/content/0/type\",\"v\":\"text\"}\n\n\
event: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/content/0/content/0/text\",\"v\":\"Hello \"}\n\n\
event: delta\ndata: {\"o\":\"BATCH\",\"p\":\"/message/content/0/content\",\"v\":[{\"o\":\"SET\",\"p\":\"/message/content/0/content/1/type\",\"v\":\"text\"},{\"o\":\"SET\",\"p\":\"/message/content/0/content/1/text\",\"v\":\"world\"}]}\n\n\
event: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/content/0/content/2/type\",\"v\":\"thinking\"}\n\n\
event: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/content/0/content/2/text\",\"v\":\"(hidden reasoning)\"}\n\n\
event: hint\ndata: {\"type\":\"notice\"}\n\n\
event: toast\ndata: {\"type\":\"business_hint\"}\n\n\
event: title\ndata: {\"title\":\"New title\"}\n\n\
event: updateSession\ndata: {\"session_id\":\"s1\"}\n\n\
event: mystery\ndata: {\"whatever\":1}\n\n\
event: finish\ndata: {\"response_message_id\":\"rsp_01\",\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":3,\"total_tokens\":14}}\n\n\
event: close\ndata: {\"close_reason\":\"success\"}\n\n";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct MockChain {
    base: String,
    challenge_calls: Arc<AtomicUsize>,
    completion_calls: Arc<AtomicUsize>,
}

async fn spawn_mock_chain(completion_sse: String, completion_status: u16) -> MockChain {
    use axum::http::HeaderMap;
    use axum::response::Response;

    let challenge_calls = Arc::new(AtomicUsize::new(0));
    let completion_calls = Arc::new(AtomicUsize::new(0));
    let cc1 = challenge_calls.clone();
    let cc2 = completion_calls.clone();

    let app = axum::Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            axum::routing::post(move |axum::Json(_b): axum::Json<serde_json::Value>| async move {
                cc1.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::from_str::<serde_json::Value>(CHALLENGE_OK).unwrap())
            }),
        )
        .route(
            "/api/v0/chat_session/create",
            axum::routing::post(|| async move {
                axum::Json(serde_json::from_str::<serde_json::Value>(SESSION_OK).unwrap())
            }),
        )
        .route(
            "/api/v0/chat/completion",
            axum::routing::post(move |headers: HeaderMap, _: axum::Json<serde_json::Value>| {
                let cc2 = cc2.clone();
                let sse = completion_sse.clone();
                async move {
                    cc2.fetch_add(1, Ordering::SeqCst);
                    // T4 wiring: the pool key must reach the upstream as the
                    // user-held web token (REA §2 Authorization: Bearer).
                    assert_eq!(
                        headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok()),
                        Some("Bearer web-token-abc"),
                        "gateway must forward the web token as Bearer authorization"
                    );
                    let mut builder = Response::builder()
                        .status(completion_status)
                        .header("content-type", "text/event-stream");
                    builder.body(axum::body::Body::from(sse)).unwrap()
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    MockChain {
        base: format!("http://{addr}"),
        challenge_calls,
        completion_calls,
    }
}

async fn spawn_gateway(upstream_base: &str) -> String {
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.providers.insert(
        "deepseek-web".to_string(),
        ProviderConfig {
            egress_pool: vec![],
            egress_strategy: "round_robin".to_string(),
            rate_limits: None,
            base_url: upstream_base.to_string(),
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

    let gateway_app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, gateway_app).await.unwrap();
    });
    format!("http://{addr}")
}

/// POST /v1/chat/completions (stream) and return the raw body text.
async fn stream_chat(gateway: &str) -> String {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{gateway}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&json!({
            "model": "deepseek-chat",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true
        }))
        .send()
        .await
        .expect("gateway must respond");
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert!(
        status.is_success(),
        "gw: {gateway} responded {status}: {text}"
    );
    text
}

/// Collect `data:` payloads of an SSE body; fails on a `[DONE]` sentinel
/// (DW-T-08: the translator must not emit one).
fn sse_data_lines(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for block in raw.split("\n\n") {
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("data:") {
                let v = v.trim_start().to_string();
                if v == "[DONE]" {
                    panic!("DW-T-08: translator must not emit data: [DONE] (wave-7 R-SS-16/A-SS-14 alignment)");
                }
                out.push(v);
            }
        }
    }
    out
}

fn parse_chunks(data_lines: &[String]) -> Vec<Value> {
    data_lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("non-JSON SSE data line {l:?}: {e}")))
        .filter(|v: &Value| v.get("choices").is_some())
        .collect()
}

fn delta_content(chunk: &Value) -> Option<&str> {
    chunk["choices"][0]["delta"]["content"].as_str()
}

fn role(chunk: &Value) -> Option<&str> {
    chunk["choices"][0]["delta"]["role"].as_str()
}

fn finish_reason(chunk: &Value) -> Option<&str> {
    chunk["choices"][0]["finish_reason"].as_str()
}

// ---------------------------------------------------------------------------
// DW-T-01 ready -> first assistant chunk with stable id prefix, no content
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_t_01_ready_emits_role_and_id_prefix() {
    let mock = spawn_mock_chain(T_STREAM.to_string(), 200).await;
    let gw = spawn_gateway(&mock.base).await;

    let lines = sse_data_lines(&stream_chat(&gw).await);
    let chunks = parse_chunks(&lines);
    assert!(!chunks.is_empty(), "DW-T-01: stream must produce chunks");

    let first = &chunks[0];
    assert_eq!(
        role(first),
        Some("assistant"),
        "DW-T-01: first chunk carries role=assistant"
    );
    let id = first["id"].as_str().unwrap_or_default();
    assert!(
        id.starts_with("chatcmpl-dsw-"),
        "DW-T-01: id must carry stable prefix chatcmpl-dsw-<response_message_id>; got {id}"
    );
    assert!(
        id.contains("rsp_01"),
        "DW-T-01: id derived from response_message_id rsp_01; got {id}"
    );
    assert_eq!(
        delta_content(first),
        None,
        "DW-T-01: ready chunk must not carry content"
    );
    let body = lines.join("\n");
    assert!(
        !body.contains("req_01"),
        "DW-T-01: request_message_id must not leak to the downstream"
    );
}

// ---------------------------------------------------------------------------
// DW-T-02 SET ops: text appended to content; non-text (thinking) excluded
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_t_02_set_op_appends_text() {
    let mock = spawn_mock_chain(T_STREAM.to_string(), 200).await;
    let gw = spawn_gateway(&mock.base).await;

    let chunks = parse_chunks(&sse_data_lines(&stream_chat(&gw).await));
    let mut content: Vec<&str> = chunks.iter().filter_map(delta_content).collect();
    // compact: drop empty
    content.retain(|c| !c.is_empty());
    assert_eq!(
        content, vec!["Hello ", "world"],
        "DW-T-02: SET text fragments appended in order (incl. BATCH recursion)"
    );
    let all = chunks.iter().map(|c| delta_content(c).unwrap_or("")).collect::<String>();
    assert!(
        !all.contains("(hidden reasoning)"),
        "DW-T-02: non-text (thinking) fragment must NOT enter visible content"
    );
}

// ---------------------------------------------------------------------------
// DW-T-03 BATCH -> recursive in-order application
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_t_03_batch_recursive_in_order() {
    let mock = spawn_mock_chain(T_STREAM.to_string(), 200).await;
    let gw = spawn_gateway(&mock.base).await;

    let chunks = parse_chunks(&sse_data_lines(&stream_chat(&gw).await));
    let content: Vec<&str> = chunks.iter().filter_map(delta_content).filter(|c| !c.is_empty()).collect();
    // BATCH [SET type, SET text "world"] must yield "world" AFTER "Hello ".
    let joined = content.join("|");
    assert_eq!(joined, "Hello |world", "DW-T-03: BATCH applied recursively in order; got {joined:?}");
}

// ---------------------------------------------------------------------------
// DW-T-04 finish -> stop chunk + usage mapping (present and absent)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_t_04_finish_usage_and_stop() {
    // usage present
    let mock = spawn_mock_chain(T_STREAM.to_string(), 200).await;
    let gw = spawn_gateway(&mock.base).await;
    let chunks = parse_chunks(&sse_data_lines(&stream_chat(&gw).await));
    let last = chunks.last().expect("at least a finish chunk");
    assert_eq!(finish_reason(last), Some("stop"), "DW-T-04: finish -> finish_reason=stop");
    let usage = &last["usage"];
    assert_eq!(usage["prompt_tokens"], 11);
    assert_eq!(usage["completion_tokens"], 3);
    assert_eq!(usage["total_tokens"], 14);

    // usage absent -> omitted, never fabricated
    let no_usage_stream = T_STREAM.replace(
        r#"{"response_message_id":"rsp_01","usage":{"prompt_tokens":11,"completion_tokens":3,"total_tokens":14}}"#,
        r#"{"response_message_id":"rsp_01"}"#,
    );
    let mock2 = spawn_mock_chain(no_usage_stream, 200).await;
    let gw2 = spawn_gateway(&mock2.base).await;
    let chunks2 = parse_chunks(&sse_data_lines(&stream_chat(&gw2).await));
    let last2 = chunks2.last().expect("finish chunk");
    assert_eq!(finish_reason(last2), Some("stop"));
    assert!(
        last2.get("usage").is_none(),
        "DW-T-04: absent upstream usage must be omitted, never fabricated"
    );
}

// ---------------------------------------------------------------------------
// DW-T-05 close_reason mapping (success / invalid_query / timeout / error /
// missing) — one stream each
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_t_05_close_reason_mapping() {
    // success: normal stop (already covered); here explicit.
    let ok_stream = "event: ready\ndata: {\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}\n\nevent: close\ndata: {\"close_reason\":\"success\"}\n\n";
    let mock = spawn_mock_chain(ok_stream.to_string(), 200).await;
    let gw = spawn_gateway(&mock.base).await;
    let chunks = parse_chunks(&sse_data_lines(&stream_chat(&gw).await));
    let last = chunks.last().expect("finish chunk");
    assert_eq!(
        finish_reason(last),
        Some("stop"),
        "DW-T-05: success close -> stop terminal (even without an upstream finish)"
    );

    // invalid_query / timeout / error: explicit failure visible downstream,
    // and never a fake success terminal.
    for (reason, expect_in_error) in [
        ("invalid_query", "invalid"),
        ("timeout", "timeout"),
        ("error", "error"),
    ] {
        let stream = format!(
            "event: ready\ndata: {{\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}}\n\nevent: close\ndata: {{\"close_reason\":\"{reason}\"}}\n\n"
        );
        let mock = spawn_mock_chain(stream, 200).await;
        let gw = spawn_gateway(&mock.base).await;
        let resp = reqwest::Client::new()
            .post(format!("{gw}/v1/chat/completions"))
            .bearer_auth("web-token-abc")
            .json(&json!({"model":"deepseek-chat","messages":[{"role":"user","content":"hi"}],"stream":true}))
            .send()
            .await
            .unwrap();
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let chunks = parse_chunks(&sse_data_lines(&text));
        assert!(
            chunks.iter().all(|c| finish_reason(c) != Some("stop")),
            "DW-T-05: {reason} close must NOT fake a success terminal"
        );
        let surfaced = !status.is_success()
            || text.to_lowercase().contains(expect_in_error)
            || text.to_lowercase().contains("error");
        assert!(
            surfaced,
            "DW-T-05: {reason} close surfaces an explicit error; status {status} body {text}"
        );
    }
}

// ---------------------------------------------------------------------------
// DW-T-06 degradation: unambiguous-restore impossible -> final message
// authoritative, one complete content output, ops debug-logged (never dropped
// silently / never fabricated)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_t_06_degradation_final_message_authoritative() {
    // Fixture: one canonical text SET ("A") plus an unknown-path SET ("B")
    // makes per-op restore ambiguous -> the translator must degrade: emit the
    // complete content exactly once (not per-op), and the un-restorable op
    // value must never surface as visible content.
    let stream = "event: ready\ndata: {\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}\n\n\
event: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/content/0/content/0/text\",\"v\":\"A\"}\n\n\
event: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/unknown/structure/0\",\"v\":\"B\"}\n\n\
event: finish\ndata: {\"response_message_id\":\"r1\"}\n\n\
event: close\ndata: {\"close_reason\":\"success\"}\n\n";
    let mock = spawn_mock_chain(stream.to_string(), 200).await;
    let gw = spawn_gateway(&mock.base).await;

    let chunks = parse_chunks(&sse_data_lines(&stream_chat(&gw).await));
    let content: Vec<&str> = chunks.iter().filter_map(delta_content).filter(|c| !c.is_empty()).collect();
    let joined = content.join("");
    assert_eq!(
        joined, "A",
        "DW-T-06: degradation outputs the complete final content once; got {joined:?}"
    );
    assert!(
        !joined.contains('B'),
        "DW-T-06: un-restorable op values must not surface as visible content"
    );
    let last = chunks.last().expect("terminal chunk");
    assert_eq!(
        finish_reason(last),
        Some("stop"),
        "DW-T-06: degraded stream still terminates well-formed"
    );
}

// ---------------------------------------------------------------------------
// DW-T-07 hint/toast/title/updateSession are non-fatal: no chunks, stream
// continues to finish
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_t_07_non_fatal_events_continue_stream() {
    let mock = spawn_mock_chain(T_STREAM.to_string(), 200).await;
    let gw = spawn_gateway(&mock.base).await;

    let chunks = parse_chunks(&sse_data_lines(&stream_chat(&gw).await));
    let last = chunks.last().expect("stream continues to a terminal");
    assert_eq!(
        finish_reason(last),
        Some("stop"),
        "DW-T-07: hint/toast/title/updateSession interleaved must not interrupt the stream"
    );
    // The content seen is only the real text (no hint/toast-derived junk).
    let content = chunks.iter().map(|c| delta_content(c).unwrap_or("")).collect::<String>();
    assert_eq!(content.contains("business_hint"), false);
    assert_eq!(content.contains("notice"), false);
}

// ---------------------------------------------------------------------------
// DW-T-08 non-event frames ignored; the translator adds no [DONE] sentinel
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_t_08_non_event_frames_ignored_no_done() {
    let stream = "event: ready\ndata: {\"response_message_id\":\"r1\",\"request_message_id\":\"q1\",\"model_type\":\"m\"}\n\n\
: keep-alive comment\n\n\
data: [DONE]\n\n\
event: delta\ndata: {\"o\":\"SET\",\"p\":\"/message/content/0/content/0/text\",\"v\":\"x\"}\n\n\
event: close\ndata: {\"close_reason\":\"success\"}\n\n";
    let mock = spawn_mock_chain(stream.to_string(), 200).await;
    let gw = spawn_gateway(&mock.base).await;

    // sse_data_lines panics on [DONE]; non-event frames must yield no chunks.
    let lines = sse_data_lines(&stream_chat(&gw).await);
    let chunks = parse_chunks(&lines);
    let content: Vec<&str> = chunks.iter().filter_map(delta_content).filter(|c| !c.is_empty()).collect();
    assert_eq!(content, vec!["x"], "DW-T-08: keep-alive/[DONE] frames ignored, text still delivered");
}

// ---------------------------------------------------------------------------
// DW-T-09 unknown event names tolerated (forward compatibility)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dw_t_09_unknown_event_tolerated() {
    let mock = spawn_mock_chain(T_STREAM.to_string(), 200).await;
    let gw = spawn_gateway(&mock.base).await;
    let chunks = parse_chunks(&sse_data_lines(&stream_chat(&gw).await));
    let last = chunks.last().expect("terminal chunk");
    assert_eq!(
        finish_reason(last),
        Some("stop"),
        "DW-T-09: the mystery event inside T_STREAM must be ignored, stream continues"
    );
}

// ---------------------------------------------------------------------------
// Gateway-level error propagation (contract §6 gateway-side notes)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gw_40012_banned_surfaces_at_gateway() {
    // The challenge endpoint answers 40012 USER_IS_BANNED. The gateway must
    // surface an explicit error to the client and must NOT retry the account
    // (banned tokens must not be re-consumed: DW-E-02 gateway note).
    use axum::http::StatusCode as St;
    use axum::response::Response as AxResponse;
    let challenge_calls = Arc::new(AtomicUsize::new(0));
    let cc = challenge_calls.clone();
    let app = axum::Router::new()
        .route(
            "/api/v0/chat_session/create",
            axum::routing::post(|| async move {
                axum::Json(serde_json::from_str::<serde_json::Value>(SESSION_OK).unwrap())
            }),
        )
        .route(
            "/api/v0/chat/create_pow_challenge",
            axum::routing::post(move |axum::Json(_): axum::Json<serde_json::Value>| async move {
                cc.fetch_add(1, Ordering::SeqCst);
                AxResponse::builder()
                    .status(St::OK)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"data":{"biz_code":40012,"biz_msg":"USER_IS_BANNED","biz_data":null}}"#,
                    ))
                    .unwrap()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let gw = spawn_gateway(&format!("http://{addr}")).await;

    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&json!({"model":"deepseek-chat","messages":[{"role":"user","content":"hi"}]}))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert!(
        !status.is_success() || text.to_lowercase().contains("banned"),
        "gw: 40012 must surface as an explicit error; status {status} body {text}"
    );
    assert_eq!(
        challenge_calls.load(Ordering::SeqCst),
        1,
        "DW-E-02: banned token consumes zero retries"
    );
}

#[tokio::test]
async fn gw_40301_invalid_pow_surfaces_at_gateway() {
    // challenge OK -> completion answers 40301 INVALID_POW_RESPONSE. The
    // gateway must surface an explicit error and not re-solve in an infinite
    // loop (DW-E-03: bounded refetch window only; repeated 40301 -> error).
    use axum::http::StatusCode as St;
    use axum::response::Response as AxResponse;
    let completion_calls = Arc::new(AtomicUsize::new(0));
    let cc = completion_calls.clone();
    let app = axum::Router::new()
        .route(
            "/api/v0/chat/create_pow_challenge",
            axum::routing::post(|axum::Json(_): axum::Json<serde_json::Value>| async move {
                axum::Json(serde_json::from_str::<serde_json::Value>(CHALLENGE_OK).unwrap())
            }),
        )
        .route(
            "/api/v0/chat_session/create",
            axum::routing::post(|| async move {
                axum::Json(serde_json::from_str::<serde_json::Value>(SESSION_OK).unwrap())
            }),
        )
        .route(
            "/api/v0/chat/completion",
            axum::routing::post(move |_: axum::Json<serde_json::Value>| {
                let cc = cc.clone();
                async move {
                    cc.fetch_add(1, Ordering::SeqCst);
                    AxResponse::builder()
                        .status(St::OK)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(
                            r#"{"data":{"biz_code":40301,"biz_msg":"INVALID_POW_RESPONSE","biz_data":null}}"#,
                        ))
                        .unwrap()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let gw = spawn_gateway(&format!("http://{addr}")).await;

    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&json!({"model":"deepseek-chat","messages":[{"role":"user","content":"hi"}]}))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert!(
        !status.is_success() || text.to_lowercase().contains("pow"),
        "gw: 40301 must surface as an explicit error; status {status} body {text}"
    );
    assert_eq!(
        completion_calls.load(Ordering::SeqCst),
        1,
        "DW-E-03: repeated 40301 must not trigger infinite re-solving"
    );
}

#[tokio::test]
async fn gw_waf_captcha_surfaces_at_gateway() {
    // Challenge response carries x-amzn-waf-action: captcha -> explicit
    // user-facing error, zero bypass attempts (DW-G-02 gateway note).
    use axum::http::StatusCode as St;
    use axum::response::Response as AxResponse;
    let challenge_calls = Arc::new(AtomicUsize::new(0));
    let cc = challenge_calls.clone();
    let app = axum::Router::new()
        .route(
            "/api/v0/chat_session/create",
            axum::routing::post(|| async move {
                axum::Json(serde_json::from_str::<serde_json::Value>(SESSION_OK).unwrap())
            }),
        )
        .route(
            "/api/v0/chat/create_pow_challenge",
            axum::routing::post(move |axum::Json(_): axum::Json<serde_json::Value>| async move {
                cc.fetch_add(1, Ordering::SeqCst);
                AxResponse::builder()
                    .status(St::OK)
                    .header("content-type", "application/json")
                    .header("x-amzn-waf-action", "captcha")
                    .body(axum::body::Body::from(CHALLENGE_OK))
                    .unwrap()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let gw = spawn_gateway(&format!("http://{addr}")).await;

    let resp = reqwest::Client::new()
        .post(format!("{gw}/v1/chat/completions"))
        .bearer_auth("web-token-abc")
        .json(&json!({"model":"deepseek-chat","messages":[{"role":"user","content":"hi"}]}))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert!(
        !status.is_success() || text.to_lowercase().contains("captcha"),
        "gw: WAF captcha must surface explicitly, never be bypassed; status {status} body {text}"
    );
    assert_eq!(
        challenge_calls.load(Ordering::SeqCst),
        1,
        "DW-G-02: captcha wall -> zero retries/bypass attempts"
    );
}