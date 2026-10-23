//! FROZEN RED-PHASE ACCEPTANCE — Wave-1b F2 (contract v5): the embedded SDK
//! (`crates/ponyllm/src/sdk.rs`) serializes same-protocol requests with a bare
//! `serde_json::to_value(req)` (lines 170 / 197 / 256), so the flattened
//! `extra` map goes straight onto the wire — the reserved-key sanitizer that
//! protects the HTTP gateway never runs on this path.
//!
//! Each test drives the REAL SDK API against a local mock upstream and asserts
//! what the upstream actually received:
//! - `chat_completion` with Chat-native upstream → metadata / store /
//!   max_output_tokens (all cross-protocol keys, i.e. they can only arrive via
//!   `extra`) must be stripped;
//! - `create_message` with Anthropic-native upstream → store / max_output_tokens
//!   / instructions stripped;
//! - `create_response` with Responses-native upstream → store / system /
//!   thinking stripped;
//! - a legal extension key survives every path (over-tightening guard);
//! - A1 end-to-end: a 32 MiB `extra` value is dropped and the call still
//!   succeeds (no panic, no upstream bloat).
//!
//! Fixtures are dummy credentials only (`sk-mock-*`), as required by the
//! secret-redaction rule.

use std::sync::{Arc, Mutex};

use axum::extract::DefaultBodyLimit;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use ponyllm::prelude::*;
use ponyllm_core::pool::UpstreamProtocol;
use serde_json::{json, Value};

type Captured = Arc<Mutex<Vec<Value>>>;

/// One mock upstream hosting all three same-protocol wire endpoints.
async fn spawn_mock_upstream() -> (String, Captured, Captured, Captured) {
    let chat_seen: Captured = Arc::new(Mutex::new(Vec::new()));
    let ant_seen: Captured = Arc::new(Mutex::new(Vec::new()));
    let resp_seen: Captured = Arc::new(Mutex::new(Vec::new()));

    let chat_sink = chat_seen.clone();
    let chat = post(move |body: String| {
        let sink = chat_sink.clone();
        async move {
            sink.lock()
                .expect("chat capture lock")
                .push(serde_json::from_str(&body).unwrap_or(Value::Null));
            Json(json!({
                "id": "chatcmpl-sdk",
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
    })
    .layer(DefaultBodyLimit::disable());

    let ant_sink = ant_seen.clone();
    let messages = post(move |body: String| {
        let sink = ant_sink.clone();
        async move {
            sink.lock()
                .expect("messages capture lock")
                .push(serde_json::from_str(&body).unwrap_or(Value::Null));
            Json(json!({
                "id": "msg_sdk",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "ok"}],
                "model": "claude-3-5-sonnet",
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {"input_tokens": 3, "output_tokens": 1}
            }))
            .into_response()
        }
    });

    let resp_sink = resp_seen.clone();
    let responses = post(move |body: String| {
        let sink = resp_sink.clone();
        async move {
            sink.lock()
                .expect("responses capture lock")
                .push(serde_json::from_str(&body).unwrap_or(Value::Null));
            Json(json!({
                "id": "resp_sdk",
                "object": "response",
                "created_at": 1,
                "status": "completed",
                "model": "gpt-4o-mini",
                "output": []
            }))
            .into_response()
        }
    });

    let app = Router::new()
        .route("/chat/v1/chat/completions", chat)
        .route("/anthropic/v1/messages", messages)
        .route("/resp/v1/responses", responses);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");
    (base, chat_seen, ant_seen, resp_seen)
}

fn gateway_for(base: &str) -> PonyGateway {
    PonyGateway::builder()
        // Same-protocol routes: the provider's native wire == the client wire.
        .add_provider(
            "chat-up",
            format!("{base}/chat"),
            "gpt-4o-mini",
            RoutingStrategy::RoundRobin,
        )
        .set_provider_protocol("chat-up", UpstreamProtocol::Chat)
        .add_provider(
            "anthropic-up",
            format!("{base}/anthropic"),
            "claude-3-5-sonnet",
            RoutingStrategy::RoundRobin,
        )
        .set_provider_protocol("anthropic-up", UpstreamProtocol::Anthropic)
        .add_provider(
            "responses-up",
            format!("{base}/resp"),
            "gpt-4o",
            RoutingStrategy::RoundRobin,
        )
        .set_provider_protocol("responses-up", UpstreamProtocol::Responses)
        .add_key("chat-up", "k-chat", "sk-mock-chat", 1, 10)
        .add_key("anthropic-up", "k-ant", "sk-mock-ant", 1, 10)
        .add_key("responses-up", "k-resp", "sk-mock-resp", 1, 10)
        .max_retries(1)
        .build()
}

fn only_body(captured: &Captured, label: &str) -> Value {
    let bodies = captured.lock().expect("capture lock").clone();
    assert_eq!(
        bodies.len(),
        1,
        "F2[{label}]: exactly one upstream attempt expected, got {}",
        bodies.len()
    );
    bodies[0].clone()
}

// ---------------------------------------------------------------------------
// F2 — SDK same-protocol paths run the reserved-key sanitizer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn f2_sdk_chat_same_protocol_strips_cross_protocol_keys() {
    let (base, chat_seen, _ant, _resp) = spawn_mock_upstream().await;
    let gateway = gateway_for(&base);

    let req: ChatCompletionRequest = serde_json::from_value(json!({
        "model": "gpt-4o-mini",
        "max_tokens": 64,
        "messages": [{"role": "user", "content": "hi"}],
        // Cross-protocol / control keys can only arrive through `extra`:
        "metadata": {"user_id": "ATTACKER_META"},
        "store": true,
        "max_output_tokens": 999999,
        // Legal extension key (over-tightening guard):
        "x_provider_ext_flag": true
    }))
    .expect("chat request deserializes");

    gateway
        .chat_completion(&req)
        .await
        .expect("SDK chat_completion must succeed");

    let wire = only_body(&chat_seen, "chat");
    assert!(
        wire.get("metadata").is_none(),
        "F2: SDK chat path must strip `metadata`; upstream body = {wire}"
    );
    assert!(
        wire.get("store").is_none(),
        "F2: SDK chat path must strip `store`; upstream body = {wire}"
    );
    assert!(
        wire.get("max_output_tokens").is_none(),
        "F2: SDK chat path must strip `max_output_tokens`; upstream body = {wire}"
    );
    assert!(
        !wire.to_string().contains("999999") && !wire.to_string().contains("ATTACKER_META"),
        "F2: injected values must never reach the SDK's upstream body; upstream body = {wire}"
    );
    assert_eq!(
        wire.get("max_tokens").and_then(Value::as_u64),
        Some(64),
        "F2: the Chat-native `max_tokens` must be untouched; upstream body = {wire}"
    );
    assert_eq!(
        wire.get("x_provider_ext_flag"),
        Some(&json!(true)),
        "F2: a legal extension key must survive the SDK path; upstream body = {wire}"
    );
}

#[tokio::test]
async fn f2_sdk_messages_same_protocol_strips_cross_protocol_keys() {
    let (base, _chat, ant_seen, _resp) = spawn_mock_upstream().await;
    let gateway = gateway_for(&base);

    let req: MessageRequest = serde_json::from_value(json!({
        "model": "claude-3-5-sonnet",
        "max_tokens": 64,
        "messages": [{"role": "user", "content": "hi"}],
        // Non-Anthropic-field keys → land in `extra`:
        "store": true,
        "max_output_tokens": 999999,
        "instructions": "ATTACKER_INSTRUCTIONS",
        "x_provider_ext_flag": true
    }))
    .expect("messages request deserializes");

    gateway
        .create_message(&req)
        .await
        .expect("SDK create_message must succeed");

    let wire = only_body(&ant_seen, "messages");
    assert!(
        wire.get("store").is_none(),
        "F2: SDK messages path must strip `store`; upstream body = {wire}"
    );
    assert!(
        wire.get("max_output_tokens").is_none(),
        "F2: SDK messages path must strip `max_output_tokens`; upstream body = {wire}"
    );
    assert!(
        wire.get("instructions").is_none(),
        "F2: SDK messages path must strip `instructions`; upstream body = {wire}"
    );
    assert!(
        !wire.to_string().contains("ATTACKER_INSTRUCTIONS"),
        "F2: injected instructions must never reach the SDK's upstream body; upstream body = {wire}"
    );
    assert_eq!(
        wire.get("x_provider_ext_flag"),
        Some(&json!(true)),
        "F2: a legal extension key must survive the SDK messages path; upstream body = {wire}"
    );
}

#[tokio::test]
async fn f2_sdk_responses_same_protocol_strips_cross_protocol_keys() {
    let (base, _chat, _ant, resp_seen) = spawn_mock_upstream().await;
    let gateway = gateway_for(&base);

    let req: CreateResponseRequest = serde_json::from_value(json!({
        "model": "gpt-4o",
        "input": "hi",
        // Non-Responses-field control keys → land in `extra`:
        "store": true,
        "system": "ATTACKER_SYSTEM",
        "thinking": {"type": "enabled", "budget_tokens": 1},
        "x_provider_ext_flag": true
    }))
    .expect("responses request deserializes");

    gateway
        .create_response(&req)
        .await
        .expect("SDK create_response must succeed");

    let wire = only_body(&resp_seen, "responses");
    assert!(
        wire.get("store").is_none(),
        "F2: SDK responses path must strip `store`; upstream body = {wire}"
    );
    assert!(
        wire.get("system").is_none(),
        "F2: SDK responses path must strip `system`; upstream body = {wire}"
    );
    assert!(
        wire.get("thinking").is_none(),
        "F2: SDK responses path must strip `thinking`; upstream body = {wire}"
    );
    assert!(
        !wire.to_string().contains("ATTACKER_SYSTEM"),
        "F2: injected system must never reach the SDK's upstream body; upstream body = {wire}"
    );
    assert_eq!(
        wire.get("x_provider_ext_flag"),
        Some(&json!(true)),
        "F2: a legal extension key must survive the SDK responses path; upstream body = {wire}"
    );
}

// ---------------------------------------------------------------------------
// A1 (end-to-end) — a 32 MiB extra value is dropped and the call still succeeds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a1_sdk_drops_32mib_extra_value_and_still_serves_the_call() {
    let (base, chat_seen, _ant, _resp) = spawn_mock_upstream().await;
    let gateway = gateway_for(&base);

    let huge = "A".repeat(32 * 1024 * 1024);
    let req: ChatCompletionRequest = serde_json::from_value(json!({
        "model": "gpt-4o-mini",
        "max_tokens": 64,
        "messages": [{"role": "user", "content": "hi"}],
        "x_huge": huge,
        "x_keep": "ok"
    }))
    .expect("chat request deserializes");

    // The call must still succeed — a hostile size may never break the request.
    gateway
        .chat_completion(&req)
        .await
        .expect("A1: a 32 MiB extra value must not fail the SDK call");

    let wire = only_body(&chat_seen, "a1-huge");
    assert!(
        wire.get("x_huge").is_none(),
        "A1: the 32 MiB extra value must be dropped before the wire; upstream body keys = {:?}",
        wire.as_object()
            .map(|m| m.keys().cloned().collect::<Vec<_>>())
    );
    assert_eq!(
        wire.get("x_keep"),
        Some(&json!("ok")),
        "A1: the in-budget neighbour must survive; upstream body = {wire}"
    );
}
