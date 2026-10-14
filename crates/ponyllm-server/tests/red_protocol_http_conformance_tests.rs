//! RED-PHASE L2-T conformance tests — **HTTP boundary** E2E for both protocols.
//!
//! Contract: `.dev-team/contracts/wave-7-protocol-conformance.md` (§1.1 `R-NS`, §1.2 `R-SS`,
//! §1.3 `A-NS`, §1.4 `A-SS`, §2 assertion table).
//!
//! Harness: the **real production router** (`ponyllm_server::create_app` over `AppState`, exactly
//! as the CLI binary builds it) is bound to a random port, in front of a deterministic mock
//! OpenAI-chat upstream that is also bound to a random port. Harness shape follows
//! `crates/ponyllm-server/tests/streaming_gateway_tests.rs` (pool registration + `AuthMode::Open`).
//!
//! These tests are written to **fail on current main** and to pass only once the frozen contract is
//! implemented. Assertions are exact JSON-path equality on the frames actually returned over the
//! wire — never "contains a key" and never "did not return an error".

use std::sync::Arc;

use axum::body::Body;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use serde_json::{json, Value};

const ROUTED_MODEL: &str = "deepseek-v4-flash";
const ANSWER: &str = "ping";
const REASONING: &str = "The user wants a single word: ping.";

// ---------------------------------------------------------------------------
// mock upstream — deterministic OpenAI-chat upstream
// ---------------------------------------------------------------------------

/// Clean turn: role -> reasoning -> answer -> finish(stop, usage) -> [DONE].
/// Deliberately carries **no** zero-length content delta, so the contract's normative event
/// sequence can be asserted exactly.
const UPSTREAM_SSE_CLEAN: &str = concat!(
    "data: {\"id\":\"chatcmpl-9f961259c79311f1\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-9f961259c79311f1\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"The user wants a single word: ping.\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-9f961259c79311f1\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ping\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-9f961259c79311f1\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":42,\"completion_tokens\":34,\"total_tokens\":76,\"prompt_tokens_details\":{\"cached_tokens\":0},\"completion_tokens_details\":{\"reasoning_tokens\":20}}}\n\n",
    "data: [DONE]\n\n",
);

/// The live 2026-10-14 shape: reasoning deltas interleaved with **zero-length** content deltas.
const UPSTREAM_SSE_WITH_EMPTY_DELTAS: &str = concat!(
    "data: {\"id\":\"chatcmpl-9f961259c79311f1\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-9f961259c79311f1\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"The user\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-9f961259c79311f1\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-9f961259c79311f1\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\" wants a single word.\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-9f961259c79311f1\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-9f961259c79311f1\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ping\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-9f961259c79311f1\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"deepseek-v4-flash\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":42,\"completion_tokens\":34,\"total_tokens\":76}}\n\n",
    "data: [DONE]\n\n",
);

fn sse_response(text: &'static str) -> Response {
    let mut resp = Response::new(Body::from(text));
    resp.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/event-stream"),
    );
    resp
}

fn mock_upstream_router() -> Router {
    Router::new().route(
        "/v1/chat/completions",
        post(|Json(req): Json<Value>| async move {
            if req.get("stream").and_then(Value::as_bool).unwrap_or(false) {
                sse_response(UPSTREAM_SSE_CLEAN).into_response()
            } else {
                Json(json!({
                    "id": "chatcmpl-9f961259c79311f1",
                    "object": "chat.completion",
                    "created": 1760417629,
                    "model": ROUTED_MODEL,
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": ANSWER,
                            "reasoning_content": REASONING
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 42,
                        "completion_tokens": 34,
                        "total_tokens": 76,
                        "prompt_tokens_details": { "cached_tokens": 0 },
                        "completion_tokens_details": { "reasoning_tokens": 20 }
                    }
                }))
                .into_response()
            }
        }),
    )
}

fn make_provider(base_url: &str, default_model: &str) -> ProviderConfig {
    ProviderConfig {
        egress_pool: vec![],
        egress_strategy: "round_robin".to_string(),
        rate_limits: None,
        base_url: base_url.to_string(),
        default_model: default_model.to_string(),
        strategy: "round_robin".to_string(),
        billing_mode: BillingMode::Metered,
        input_price: 0.50,
        cached_price: 0.25,
        output_price: 1.00,
        models: vec![],
        model_specs: Vec::new(),
        default_protocol: None,
        chat_url: None,
        responses_url: None,
        messages_url: None,
        proxy: None,
        timeout_secs: None,
        ttfb_timeout_secs: None,
    }
}

/// Bind the mock upstream and the **real** gateway router on random ports; return the gateway URL.
async fn spawn_gateway() -> String {
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock_upstream_router())
            .await
            .unwrap();
    });

    let pool = Arc::new(KeyPool::new("deepseek", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-mock-key-123456", 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.providers.insert(
        "deepseek".to_string(),
        make_provider(&format!("http://{}", upstream_addr), ROUTED_MODEL),
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek", pool);

    let app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, app).await.unwrap();
    });
    format!("http://{}", gateway_addr)
}

// ---------------------------------------------------------------------------
// assertions
// ---------------------------------------------------------------------------

#[track_caller]
fn assert_path_eq(body: &Value, ptr: &str, expected: Value, id: &str) {
    let actual = body.pointer(ptr);
    assert_eq!(
        actual,
        Some(&expected),
        "{id}: wire path `{ptr}` must equal {expected}; full body: {body}"
    );
}

#[track_caller]
fn assert_int_ge(body: &Value, ptr: &str, min: i64, id: &str) {
    let actual = body.pointer(ptr).and_then(Value::as_i64);
    assert!(
        actual.is_some_and(|v| v >= min),
        "{id}: wire path `{ptr}` must be an integer >= {min}; got {:?}; full body: {body}",
        body.pointer(ptr)
    );
}

#[track_caller]
fn assert_string_matching(body: &Value, ptr: &str, prefix: &str, min_tail: usize, id: &str) {
    let actual = body.pointer(ptr).and_then(Value::as_str);
    let ok = actual.is_some_and(|s| s.starts_with(prefix) && s.len() >= prefix.len() + min_tail);
    assert!(
        ok,
        "{id}: wire path `{ptr}` must be a string starting with `{prefix}` and >= {} chars total; got {actual:?}; full body: {body}",
        prefix.len() + min_tail
    );
}

/// Parse an SSE body into `(event_name, data_json)` pairs, failing loudly on any `data:` line that
/// is not valid JSON (this is the "no malformed JSON on the wire" gate).
#[track_caller]
fn parse_sse(body: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for block in body.split("\n\n") {
        let block = block.trim_matches(|c| c == '\n' || c == '\r');
        if block.is_empty() {
            continue;
        }
        let mut event: Option<String> = None;
        let mut data: Option<String> = None;
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("event:") {
                event = Some(v.trim().to_string());
            } else if let Some(v) = line.strip_prefix("data:") {
                data = Some(v.trim().to_string());
            }
        }
        match (event, data) {
            (Some(e), Some(d)) => {
                let parsed: Value = serde_json::from_str(&d).unwrap_or_else(|err| {
                    panic!("malformed JSON in SSE frame `{e}`: {err}; raw data line: {d}")
                });
                out.push((e, parsed));
            }
            (Some(e), None) => panic!("SSE frame `{e}` has no `data:` line; raw block: {block}"),
            (None, _) => panic!("SSE block has no `event:` line; raw block: {block}"),
        }
    }
    out
}

/// R-SS-17 / A-SS-13 — every frame's SSE event name equals its JSON `type`.
#[track_caller]
fn assert_frame_event_name_matches_type(frames: &[(String, Value)], id: &str) {
    for (name, data) in frames {
        let ty = data.pointer("/type").and_then(Value::as_str);
        assert_eq!(
            ty,
            Some(name.as_str()),
            "{id}: SSE `event: {name}` must match `data.type`; full frame data: {data}"
        );
    }
}

#[track_caller]
fn assert_no_done_sentinel(body: &str, id: &str) {
    assert!(
        !body.contains("[DONE]"),
        "{id}: the stream MUST NOT contain a `[DONE]` sentinel; raw body: {body}"
    );
}

// ===========================================================================
// R-NS — POST /v1/responses, non-stream, over the real router
// ===========================================================================

/// R-NS-01..R-NS-21 — the exact `POST /v1/responses` body returned by the real router.
#[tokio::test]
async fn r_ns_01_to_21_http_responses_nonstream_exact_body() {
    let base = spawn_gateway().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/responses", base))
        .json(&json!({"model": ROUTED_MODEL, "input": "say ping"}))
        .send()
        .await
        .expect("request must complete");

    assert_eq!(resp.status(), 200, "R-NS: non-stream responses must be 200");
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/json"),
        "R-NS: non-stream responses must be application/json"
    );
    let body: Value = resp.json().await.expect("body must be JSON");

    assert_string_matching(&body, "/id", "resp_", 8, "R-NS-01");
    assert_path_eq(&body, "/object", Value::String("response".into()), "R-NS-02");
    assert_int_ge(&body, "/created_at", 1, "R-NS-03");
    assert_path_eq(&body, "/status", Value::String("completed".into()), "R-NS-04");
    assert_path_eq(
        &body,
        "/model",
        Value::String(ROUTED_MODEL.into()),
        "R-NS-05",
    );
    assert!(
        body.pointer("/output").is_some_and(Value::is_array),
        "R-NS-06: `/output` must be an array; body: {body}"
    );

    let msg_idx = body
        .pointer("/output")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .position(|it| it.get("type").and_then(Value::as_str) == Some("message"))
        })
        .unwrap_or_else(|| panic!("R-NS-07: no `message` output item; body: {body}"));

    assert_path_eq(
        &body,
        &format!("/output/{msg_idx}/type"),
        Value::String("message".into()),
        "R-NS-07",
    );
    assert_string_matching(&body, &format!("/output/{msg_idx}/id"), "", 1, "R-NS-08");
    assert_path_eq(
        &body,
        &format!("/output/{msg_idx}/status"),
        Value::String("completed".into()),
        "R-NS-09",
    );
    assert_path_eq(
        &body,
        &format!("/output/{msg_idx}/role"),
        Value::String("assistant".into()),
        "R-NS-10",
    );
    assert_path_eq(
        &body,
        &format!("/output/{msg_idx}/content/0/type"),
        Value::String("output_text".into()),
        "R-NS-11",
    );
    assert_path_eq(
        &body,
        &format!("/output/{msg_idx}/content/0/text"),
        Value::String(ANSWER.into()),
        "R-NS-11",
    );
    assert_path_eq(
        &body,
        &format!("/output/{msg_idx}/content/0/annotations"),
        Value::Array(vec![]),
        "R-NS-12",
    );

    let reasoning_idx = body
        .pointer("/output")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .position(|it| it.get("type").and_then(Value::as_str) == Some("reasoning"))
        })
        .unwrap_or_else(|| panic!("R-NS-13: no top-level `reasoning` output item; body: {body}"));
    assert_path_eq(
        &body,
        &format!("/output/{reasoning_idx}/summary/0/type"),
        Value::String("summary_text".into()),
        "R-NS-14",
    );

    let part_types: Vec<String> = body
        .pointer("/output")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|it| it.get("type").and_then(Value::as_str) == Some("message"))
                .filter_map(|it| it.get("content").and_then(Value::as_array))
                .flatten()
                .filter_map(|p| p.get("type").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !part_types.is_empty(),
        "R-NS-06/R-NS-16: the assistant message must carry at least one content part (guards against a vacuous pass); body: {body}"
    );
    assert!(
        !part_types.iter().any(|t| t == "reasoning" || t == "input_text"),
        "R-NS-16: message content parts must be `output_text`/`refusal` only; got {part_types:?}; body: {body}"
    );

    // The mock upstream reports cached_tokens: 0 and reasoning_tokens: 20, so these are exact.
    assert_path_eq(
        &body,
        "/usage/input_tokens_details/cached_tokens",
        Value::from(0u64),
        "R-NS-17",
    );
    assert_path_eq(
        &body,
        "/usage/output_tokens_details/reasoning_tokens",
        Value::from(20u64),
        "R-NS-18",
    );
    assert_path_eq(&body, "/usage/input_tokens", Value::from(42u64), "R-NS-19");
    assert_path_eq(&body, "/usage/output_tokens", Value::from(34u64), "R-NS-19");
    assert_path_eq(&body, "/usage/total_tokens", Value::from(76u64), "R-NS-19");
    assert_path_eq(&body, "/error", Value::Null, "R-NS-20");
    assert_path_eq(&body, "/incomplete_details", Value::Null, "R-NS-21");
}

// ===========================================================================
// R-SS — POST /v1/responses, stream:true, over the real router
// ===========================================================================

/// R-SS-01..R-SS-20 — framing, full event sequence and terminal payload of the SSE stream returned
/// by the real router for `POST /v1/responses` with `"stream": true`.
#[tokio::test]
async fn r_ss_01_to_20_http_responses_stream_frames() {
    let base = spawn_gateway().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/responses", base))
        .json(&json!({"model": ROUTED_MODEL, "input": "say ping", "stream": true}))
        .send()
        .await
        .expect("request must complete");

    assert_eq!(resp.status(), 200, "R-SS-20: streaming responses must be 200");
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream"),
        "R-SS-20: streaming responses must be text/event-stream"
    );
    let raw = resp.text().await.expect("stream body must be readable");

    assert_no_done_sentinel(&raw, "R-SS-16");

    let mut frames = parse_sse(&raw);
    assert_frame_event_name_matches_type(&frames, "R-SS-17");
    // `ping` frames MAY appear anywhere (contract §1.4/§1.2 framing note); they carry no state.
    frames.retain(|(name, _)| name != "ping");

    let names: Vec<&str> = frames.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "response.created",
            "response.in_progress",
            "response.output_item.added",
            "response.reasoning_summary_part.added",
            "response.reasoning_summary_text.delta",
            "response.reasoning_summary_text.done",
            "response.reasoning_summary_part.done",
            "response.output_item.done",
            "response.output_item.added",
            "response.content_part.added",
            "response.output_text.delta",
            "response.output_text.done",
            "response.content_part.done",
            "response.output_item.done",
            "response.completed",
        ],
        "R-SS-01..R-SS-13: SSE event sequence over the wire; raw body: {raw}"
    );

    // R-SS-04 — sequence_number on every frame, strictly increasing.
    let numbers: Vec<i64> = frames
        .iter()
        .map(|(name, data)| {
            data.pointer("/sequence_number")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    panic!("R-SS-04: frame `{name}` must carry an integer `sequence_number`: {data}")
                })
        })
        .collect();
    assert!(
        numbers.windows(2).all(|w| w[1] > w[0]),
        "R-SS-04: `sequence_number` must strictly increase across the stream; got {numbers:?}"
    );

    // R-SS-02 — response.created carries status in_progress.
    assert_path_eq(
        &frames[0].1,
        "/response/status",
        Value::String("in_progress".into()),
        "R-SS-02",
    );

    // R-SS-18 / R-SS-19 — stable, resp_-prefixed id.
    let created_id = frames[0]
        .1
        .pointer("/response/id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("R-SS-19: no `/response/id` in response.created"))
        .to_string();
    assert!(
        created_id.starts_with("resp_"),
        "R-SS-19: response id must be `resp_`-prefixed; got `{created_id}`"
    );
    let terminal = &frames.last().expect("stream must not be empty").1;
    assert_path_eq(
        terminal,
        "/response/id",
        Value::String(created_id),
        "R-SS-18",
    );

    // R-SS-13 / R-SS-14 / R-SS-15 — terminal payload.
    let output = terminal
        .pointer("/response/output")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("R-SS-12: terminal `/response/output` must be an array: {terminal}"));
    let message = output
        .iter()
        .find(|it| it.get("type").and_then(Value::as_str) == Some("message"))
        .unwrap_or_else(|| panic!("R-SS-12: terminal output must contain a message item: {terminal}"));
    assert_path_eq(
        message,
        "/content/0/type",
        Value::String("output_text".into()),
        "R-SS-13",
    );
    assert_path_eq(
        terminal,
        "/response/usage/input_tokens",
        Value::from(42u64),
        "R-SS-14",
    );
    assert_path_eq(
        terminal,
        "/response/usage/output_tokens",
        Value::from(34u64),
        "R-SS-14",
    );
    // The mock upstream's terminal usage chunk reports cached_tokens: 0 and reasoning_tokens: 20,
    // so both detail values are asserted exactly.
    assert_path_eq(
        terminal,
        "/response/usage/input_tokens_details/cached_tokens",
        Value::from(0u64),
        "R-SS-15",
    );
    assert_path_eq(
        terminal,
        "/response/usage/output_tokens_details/reasoning_tokens",
        Value::from(20u64),
        "R-SS-15",
    );
}

/// R-SS-07 — over the wire, `response.output_text.delta` carries `item_id`/`output_index`/
/// `content_index`/`delta`/`logprobs` and MUST NOT carry a `response_id` field.
#[tokio::test]
async fn r_ss_07_http_output_text_delta_frame_fields() {
    let base = spawn_gateway().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/responses", base))
        .json(&json!({"model": ROUTED_MODEL, "input": "say ping", "stream": true}))
        .send()
        .await
        .expect("request must complete");
    let raw = resp.text().await.expect("stream body must be readable");

    let frames = parse_sse(&raw);
    let (name, delta) = frames
        .iter()
        .find(|(name, _)| name == "response.output_text.delta")
        .unwrap_or_else(|| {
            panic!("R-SS-07: no `response.output_text.delta` frame on the wire; raw body: {raw}")
        });

    assert_eq!(name, "response.output_text.delta");
    assert_path_eq(delta, "/delta", Value::String(ANSWER.into()), "R-SS-07");
    assert_int_ge(delta, "/output_index", 0, "R-SS-07");
    assert_int_ge(delta, "/content_index", 0, "R-SS-07");
    assert_path_eq(delta, "/logprobs", Value::Array(vec![]), "R-SS-07");
    assert!(
        delta.pointer("/response_id").is_none(),
        "R-SS-07: `response.output_text.delta` MUST NOT carry a `response_id` field; frame: {delta}"
    );
    assert!(
        delta
            .pointer("/item_id")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty()),
        "R-SS-07: `/item_id` must be a non-empty string; frame: {delta}"
    );
}

// ===========================================================================
// A-NS — POST /v1/messages, non-stream, over the real router
// ===========================================================================

/// A-NS-01..A-NS-13 — the exact `POST /v1/messages` body returned by the real router.
#[tokio::test]
async fn a_ns_01_to_13_http_messages_nonstream_exact_body() {
    let base = spawn_gateway().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/messages", base))
        .header("x-api-key", "sk-mock-key-123456")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": ROUTED_MODEL,
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "say ping"}]
        }))
        .send()
        .await
        .expect("request must complete");

    assert_eq!(resp.status(), 200, "A-NS: non-stream messages must be 200");
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/json"),
        "A-NS: non-stream messages must be application/json"
    );
    let body: Value = resp.json().await.expect("body must be JSON");

    assert_string_matching(&body, "/id", "msg_", 1, "A-NS-01");
    assert_path_eq(&body, "/type", Value::String("message".into()), "A-NS-02");
    assert_path_eq(&body, "/role", Value::String("assistant".into()), "A-NS-03");
    assert!(
        body.pointer("/content").is_some_and(Value::is_array),
        "A-NS-04: `/content` must be an array; body: {body}"
    );
    assert_path_eq(
        &body,
        "/content/0/type",
        Value::String("thinking".into()),
        "A-NS-13",
    );
    assert_path_eq(
        &body,
        "/content/0/thinking",
        Value::String(REASONING.into()),
        "A-NS-13",
    );
    assert_path_eq(
        &body,
        "/content/0/signature",
        Value::String(String::new()),
        "A-NS-05",
    );
    assert_path_eq(
        &body,
        "/content/1/type",
        Value::String("text".into()),
        "A-NS-13",
    );
    assert_path_eq(
        &body,
        "/content/1/text",
        Value::String(ANSWER.into()),
        "A-NS-13",
    );

    let stop_reason = body
        .pointer("/stop_reason")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("A-NS-06: `/stop_reason` must be a non-null string; body: {body}"));
    const ALLOWED: [&str; 7] = [
        "end_turn",
        "max_tokens",
        "stop_sequence",
        "tool_use",
        "pause_turn",
        "refusal",
        "model_context_window_exceeded",
    ];
    assert!(
        ALLOWED.contains(&stop_reason),
        "A-NS-06: `/stop_reason` must be one of {ALLOWED:?}; got `{stop_reason}`; body: {body}"
    );
    assert_path_eq(&body, "/stop_sequence", Value::Null, "A-NS-07");
    assert_int_ge(&body, "/usage/input_tokens", 1, "A-NS-08");
    assert_int_ge(&body, "/usage/output_tokens", 1, "A-NS-09");
    assert_int_ge(
        &body,
        "/usage/cache_creation_input_tokens",
        0,
        "A-NS-10",
    );
    assert_int_ge(&body, "/usage/cache_read_input_tokens", 0, "A-NS-11");
    assert_path_eq(
        &body,
        "/model",
        Value::String(ROUTED_MODEL.into()),
        "A-NS-12",
    );
}

// ===========================================================================
// A-SS — POST /v1/messages, stream:true, over the real router
// ===========================================================================

/// A-SS-01..A-SS-14 — framing, full event sequence and terminal payload of the SSE stream returned
/// by the real router for `POST /v1/messages` with `"stream": true`.
#[tokio::test]
async fn a_ss_01_to_14_http_messages_stream_frames() {
    let base = spawn_gateway().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/messages", base))
        .header("x-api-key", "sk-mock-key-123456")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": ROUTED_MODEL,
            "max_tokens": 64,
            "stream": true,
            "messages": [{"role": "user", "content": "say ping"}]
        }))
        .send()
        .await
        .expect("request must complete");

    assert_eq!(resp.status(), 200, "A-SS: streaming messages must be 200");
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream"),
        "A-SS: streaming messages must be text/event-stream"
    );
    let raw = resp.text().await.expect("stream body must be readable");

    assert_no_done_sentinel(&raw, "A-SS-14");

    let mut frames = parse_sse(&raw);
    assert_frame_event_name_matches_type(&frames, "A-SS-13");
    // `ping` events MAY appear anywhere (contract §1.4).
    frames.retain(|(name, _)| name != "ping");

    let names: Vec<&str> = frames.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ],
        "A-SS-01/A-SS-07/A-SS-09/A-SS-12: SSE event sequence over the wire; raw body: {raw}"
    );

    // A-SS-01..A-SS-06 — message_start envelope.
    // A-SS-02 (Lead ruling, contract v2): `message_start` is emitted on the FIRST upstream chunk,
    // before any upstream usage exists — a proxy cannot know the real prompt count there. This path
    // only requires the key to be PRESENT and an integer >= 0; the real cumulative prompt count is
    // asserted on `message_delta` instead (A-SS-17, below).
    assert_int_ge(&frames[0].1, "/message/usage/input_tokens", 0, "A-SS-02");
    assert_int_ge(&frames[0].1, "/message/usage/output_tokens", 1, "A-SS-03");
    assert_path_eq(&frames[0].1, "/message/stop_reason", Value::Null, "A-SS-04");
    assert_path_eq(&frames[0].1, "/message/stop_sequence", Value::Null, "A-SS-05");
    assert_string_matching(&frames[0].1, "/message/id", "msg_", 1, "A-SS-06");

    // A-SS-07 — one start/stop per block, indices matching the final content array.
    let starts: Vec<i64> = frames
        .iter()
        .filter(|(n, _)| n == "content_block_start")
        .filter_map(|(_, d)| d.pointer("/index").and_then(Value::as_i64))
        .collect();
    let stops: Vec<i64> = frames
        .iter()
        .filter(|(n, _)| n == "content_block_stop")
        .filter_map(|(_, d)| d.pointer("/index").and_then(Value::as_i64))
        .collect();
    assert_eq!(
        starts,
        vec![0, 1],
        "A-SS-07: exactly two content blocks (thinking, text) with indices [0, 1]; raw body: {raw}"
    );
    assert_eq!(
        stops,
        vec![0, 1],
        "A-SS-07: exactly one content_block_stop per block with matching indices; raw body: {raw}"
    );

    // A-SS-08 / A-SS-09 — thinking block shape and the single signature_delta before its stop.
    let thinking_start = frames
        .iter()
        .find(|(n, d)| {
            n == "content_block_start"
                && d.pointer("/content_block/type").and_then(Value::as_str) == Some("thinking")
        })
        .unwrap_or_else(|| panic!("A-SS-08: no thinking content_block_start; raw body: {raw}"));
    assert_path_eq(
        &thinking_start.1,
        "/content_block",
        json!({"type": "thinking", "thinking": "", "signature": ""}),
        "A-SS-08",
    );
    let sig_positions: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, (n, d))| {
            n == "content_block_delta"
                && d.pointer("/delta/type").and_then(Value::as_str) == Some("signature_delta")
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        sig_positions.len(),
        1,
        "A-SS-09: exactly one signature_delta must be emitted; got {sig_positions:?}; raw body: {raw}"
    );
    assert_eq!(
        frames[sig_positions[0] + 1].0,
        "content_block_stop",
        "A-SS-09: signature_delta must be immediately followed by content_block_stop; raw body: {raw}"
    );

    // A-SS-10 / A-SS-11 / A-SS-17 — message_delta payload.
    let message_delta = frames
        .iter()
        .find(|(n, _)| n == "message_delta")
        .map(|(_, d)| d)
        .unwrap_or_else(|| panic!("A-SS-10: no message_delta frame; raw body: {raw}"));
    assert_path_eq(
        message_delta,
        "/delta/stop_sequence",
        Value::Null,
        "A-SS-10",
    );
    assert_path_eq(
        message_delta,
        "/delta/stop_reason",
        Value::String("end_turn".into()),
        "A-SS-10",
    );
    // A-SS-17 — the real cumulative prompt token count belongs here (not on message_start).
    assert_path_eq(
        message_delta,
        "/usage/input_tokens",
        Value::from(42u64),
        "A-SS-17",
    );
    assert_path_eq(
        message_delta,
        "/usage/output_tokens",
        Value::from(34u64),
        "A-SS-11",
    );

    // A-SS-12 — terminal event.
    assert_eq!(
        frames.last().expect("stream must not be empty").0,
        "message_stop",
        "A-SS-12: message_stop must be the last event; raw body: {raw}"
    );
}

/// A-SS-15 — the live interleaved upstream shape must not put zero-length `text_delta` frames on
/// the wire, and must not churn content blocks.
#[tokio::test]
async fn a_ss_15_http_no_empty_text_deltas_and_no_block_churn() {
    // This test needs the interleaved upstream, so it boots its own gateway over that fixture.
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    let upstream = Router::new().route(
        "/v1/chat/completions",
        post(|_: Json<Value>| async { sse_response(UPSTREAM_SSE_WITH_EMPTY_DELTAS) }),
    );
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new("deepseek", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-mock-key-123456", 1, 10));
    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.providers.insert(
        "deepseek".to_string(),
        make_provider(&format!("http://{}", upstream_addr), ROUTED_MODEL),
    );
    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek", pool);
    let app = create_app(state);
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = gateway_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/messages", gateway_addr))
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": ROUTED_MODEL,
            "max_tokens": 64,
            "stream": true,
            "messages": [{"role": "user", "content": "say ping"}]
        }))
        .send()
        .await
        .expect("request must complete");
    assert_eq!(resp.status(), 200);
    let raw = resp.text().await.expect("stream body must be readable");

    let frames = parse_sse(&raw);

    let empty_deltas: Vec<&Value> = frames
        .iter()
        .filter(|(n, d)| {
            n == "content_block_delta"
                && d.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta")
                && d.pointer("/delta/text").and_then(Value::as_str) == Some("")
        })
        .map(|(_, d)| d)
        .collect();
    assert!(
        empty_deltas.is_empty(),
        "A-SS-15: zero-length text_delta frames MUST NOT be emitted; got {} of them; raw body: {raw}",
        empty_deltas.len()
    );

    let starts: Vec<i64> = frames
        .iter()
        .filter(|(n, _)| n == "content_block_start")
        .filter_map(|(_, d)| d.pointer("/index").and_then(Value::as_i64))
        .collect();
    assert_eq!(
        starts,
        vec![0, 1],
        "A-SS-07: the interleaved upstream shape must still yield exactly two content blocks; got {starts:?}; raw body: {raw}"
    );
}
