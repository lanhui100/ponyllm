//! RED-PHASE **L2-AT** adversarial conformance tests — deterministic, no sleep-plus-probability.
//!
//! Contract: `.dev-team/contracts/wave-7-protocol-conformance.md` (§1.1–§1.4 target shapes,
//! §2 assertion table). Adversarial classes required by the wave-7 task:
//!
//! (a) an upstream chat stream that ends **without a `finish_reason`**, and one that **aborts
//!     mid-stream** → the gateway must still emit a complete, well-formed terminal event set and
//!     must never emit a non-official content-part type.
//! (b) oversized / malformed upstream payloads — an 80 KiB reasoning delta (above the gateway's
//!     64 KiB SSE frame cap), a zero-length delta, an unparseable frame, and a multi-byte UTF-8
//!     character split across two TCP writes → no malformed JSON on the client wire, no panic.
//! (c) concurrency — exactly 8 simultaneous `/v1/responses` and 8 simultaneous `/v1/messages`
//!     streams against **one** gateway instance, join-all, asserting no cross-request id or
//!     `sequence_number` bleed.
//!
//! Determinism: every fixture uses fixed counts and fixed byte sequences. The TCP write split in
//! (b) is forced through **HTTP chunked transfer-encoding** — the chunked decoder yields one body
//! chunk per write, so the boundary is structural, not timing-dependent. No `sleep` is used as a
//! synchronisation or assertion mechanism anywhere in this file.
//!
//! Assertions are exact JSON-path equality on the frames actually returned over the wire.

use std::sync::Arc;

use axum::body::Body;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ProviderConfig};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ROUTED_MODEL: &str = "deepseek-v4-flash";
const ANSWER: &str = "ping";

// ---------------------------------------------------------------------------
// assertion helpers
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
fn assert_string_matching(body: &Value, ptr: &str, prefix: &str, min_tail: usize, id: &str) {
    let actual = body.pointer(ptr).and_then(Value::as_str);
    let ok = actual.is_some_and(|s| s.starts_with(prefix) && s.len() >= prefix.len() + min_tail);
    assert!(
        ok,
        "{id}: wire path `{ptr}` must be a string starting with `{prefix}` and >= {} chars total; got {actual:?}; full body: {body}",
        prefix.len() + min_tail
    );
}

/// Parse an SSE body into `(event_name, data_json)` pairs. Fails loudly on any `data:` line that is
/// not valid JSON — this is the "no malformed JSON on the wire" gate for every adversarial case.
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
                    panic!("malformed JSON on the client wire in SSE frame `{e}`: {err}; raw data line: {d}")
                });
                out.push((e, parsed));
            }
            (Some(e), None) => panic!("SSE frame `{e}` has no `data:` line; raw block: {block}"),
            (None, _) => panic!("SSE block has no `event:` line; raw block: {block}"),
        }
    }
    out
}

/// Read a streaming body, turning a truncated/aborted client body into a contract failure rather
/// than a swallowed error.
#[track_caller]
fn read_stream_body(raw: Result<String, reqwest::Error>, id: &str) -> String {
    match raw {
        Ok(t) => t,
        Err(e) => panic!(
            "{id}: the client-facing stream must stay well-formed even when the upstream aborts; \
             reading the response body failed: {e}"
        ),
    }
}

// ---------------------------------------------------------------------------
// mock upstreams
// ---------------------------------------------------------------------------

fn sse_response(text: String) -> Response {
    let mut resp = Response::new(Body::from(text));
    resp.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/event-stream"),
    );
    resp
}

fn chat_chunk(id: &str, delta: &str, finish: Option<&str>, usage: Option<&str>) -> String {
    let finish_json = match finish {
        Some(f) => format!("\"{f}\""),
        None => "null".to_string(),
    };
    let usage_json = match usage {
        Some(u) => format!(",\"usage\":{u}"),
        None => String::new(),
    };
    format!(
        "data: {{\"id\":\"{id}\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"{ROUTED_MODEL}\",\"choices\":[{{\"index\":0,\"delta\":{delta},\"finish_reason\":{finish_json}}}]}}{usage_json}\n\n"
    )
}

/// One SSE `data:` frame for a chat chunk carrying an explicit **multi-choice** `choices` array.
///
/// A single upstream chunk may legitimately carry more than one choice; the per-choice FSM loops
/// must therefore re-check their terminal state mid-chunk (see the `a3` regression tests).
fn chat_chunk_multi(id: &str, choices_json: &str, usage_json: &str) -> String {
    format!(
        "data: {{\"id\":\"{id}\",\"object\":\"chat.completion.chunk\",\"created\":1760417629,\"model\":\"{ROUTED_MODEL}\",\"choices\":{choices_json}{usage_json}}}\n\n"
    )
}

/// Upstream that streams a turn and then **ends the SSE body without any `finish_reason`** and
/// without a `[DONE]` sentinel (clean EOF).
fn eof_without_finish_reason_router() -> Router {
    let body = format!(
        "{}{}{}{}",
        chat_chunk("chatcmpl-eof", "{\"role\":\"assistant\"}", None, None),
        chat_chunk("chatcmpl-eof", "{\"reasoning_content\":\"partial thought\"}", None, None),
        chat_chunk("chatcmpl-eof", "{\"content\":\"\"}", None, None),
        chat_chunk("chatcmpl-eof", "{\"content\":\"ping\"}", None, None),
    );
    Router::new().route(
        "/v1/chat/completions",
        post(move |_: Json<Value>| {
            let body = body.clone();
            async move { sse_response(body) }
        }),
    )
}

/// Drain one HTTP request (headers + `content-length` body) so the gateway is never answered while
/// it is still writing its request.
async fn drain_request(sock: &mut tokio::net::TcpStream) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
            let content_length = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            if buf.len() >= pos + 4 + content_length {
                return;
            }
        }
        match sock.read(&mut tmp).await {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return,
        }
    }
}

/// Raw HTTP/1.1 upstream that writes `parts` as **separate chunked-transfer-encoding chunks**.
///
/// * `terminate_chunked == true`  → clean end of body (a well-formed but `finish_reason`-less stream).
/// * `terminate_chunked == false` → the socket is dropped without the terminating `0\r\n\r\n`
///   chunk, which the HTTP client observes as an **aborted** (error) stream, not a clean EOF.
async fn spawn_raw_chunked_upstream(parts: Vec<Vec<u8>>, terminate_chunked: bool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let parts = parts.clone();
            tokio::spawn(async move {
                drain_request(&mut sock).await;
                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                for part in &parts {
                    if sock
                        .write_all(format!("{:x}\r\n", part.len()).as_bytes())
                        .await
                        .is_err()
                    {
                        return;
                    }
                    if sock.write_all(part).await.is_err() {
                        return;
                    }
                    if sock.write_all(b"\r\n").await.is_err() {
                        return;
                    }
                    if sock.flush().await.is_err() {
                        return;
                    }
                }
                if terminate_chunked {
                    let _ = sock.write_all(b"0\r\n\r\n").await;
                    let _ = sock.flush().await;
                }
                // When `terminate_chunked` is false the socket drops here: an abrupt mid-stream
                // abort that the HTTP client reports as a transport error.
            });
        }
    });
    format!("http://{}", addr)
}

/// Assert the stream's terminal event is one of `allowed` (`allowed[0]` is the success variant).
///
/// Contract v2 (Lead ruling): on an aborted upstream the gateway MUST emit exactly one terminal
/// event, and it may legitimately be the *failure* variant (`error` / `response.failed`) rather than
/// a success terminal. Failure-event payload schemas are explicitly out of scope — the payload need
/// only carry `type` plus a non-empty message at `failure_message_ptr`.
#[track_caller]
fn assert_terminal_event<'a>(
    frames: &'a [(String, Value)],
    allowed: &[&str],
    failure_message_ptr: &str,
    id: &str,
) -> &'a (String, Value) {
    let last = frames.last().unwrap_or_else(|| {
        panic!("{id}: the stream must not be empty — no terminal event was emitted at all")
    });
    assert!(
        allowed.contains(&last.0.as_str()),
        "{id}: the terminal event must be one of {allowed:?} (never a body truncated with no \
         terminal event); got `{}`; frames: {frames:#?}",
        last.0
    );
    if last.0 != allowed[0] {
        let msg = last.1.pointer(failure_message_ptr).and_then(Value::as_str);
        assert!(
            msg.is_some_and(|m| !m.trim().is_empty()),
            "{id}: the `{}` terminal event must carry a non-empty message at \
             `{failure_message_ptr}`; got {:?}; frame: {}",
            last.0,
            last.1.pointer(failure_message_ptr),
            last.1
        );
    }
    last
}

/// Official Anthropic `ContentBlock` `type` values (Messages API / streaming event list). Used to
/// prove no gateway-local / cross-protocol content-part type leaks onto the abort path.
const OFFICIAL_ANTHROPIC_BLOCK_TYPES: [&str; 10] = [
    "text",
    "thinking",
    "redacted_thinking",
    "tool_use",
    "tool_result",
    "image",
    "document",
    "search_result",
    "server_tool_use",
    "web_search_tool_result",
];

/// Every `content_block_start.content_block.type` observed in the stream.
fn anthropic_block_types(frames: &[(String, Value)]) -> Vec<String> {
    frames
        .iter()
        .filter(|(n, _)| n == "content_block_start")
        .filter_map(|(_, d)| d.pointer("/content_block/type").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// gateway harness
// ---------------------------------------------------------------------------

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

/// Boot the real gateway router in front of an already-running upstream URL.
async fn spawn_gateway_for(upstream_base: &str) -> String {
    let pool = Arc::new(KeyPool::new("deepseek", RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("k1", "sk-mock-key-123456", 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = ponyllm_config::AuthMode::Open;
    config.providers.insert(
        "deepseek".to_string(),
        make_provider(upstream_base, ROUTED_MODEL),
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool("deepseek", pool);

    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{}", addr)
}

/// Boot an axum upstream router and return both the upstream base URL and the gateway base URL.
async fn spawn_axum_upstream_and_gateway(router: Router) -> String {
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, router).await.unwrap();
    });
    spawn_gateway_for(&format!("http://{}", upstream_addr)).await
}

async fn post_responses_stream(base: &str, input: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .json(&json!({"model": ROUTED_MODEL, "input": input, "stream": true}))
        .send()
        .await
        .expect("gateway must accept the request")
}

async fn post_messages_stream(base: &str, text: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}/v1/messages"))
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": ROUTED_MODEL,
            "max_tokens": 64,
            "stream": true,
            "messages": [{"role": "user", "content": text}]
        }))
        .send()
        .await
        .expect("gateway must accept the request")
}

/// Every content-part `type` that appears inside a **`message`** output item anywhere in the stream.
///
/// Scoping rules (deliberately narrow — a content part is only a content part when it belongs to a
/// message item's `content` array):
/// * `/response/output[type=="message"].content[]` — the terminal aggregate.
/// * `/item.content[]` on a frame whose `item.type == "message"` — `response.output_item.added` /
///   `.done`. The reasoning item's own `output_item.*` frames carry `item.type == "reasoning"` and
///   are excluded here.
/// * `/part` — ONLY on `response.content_part.*` frames, which are the message item's own
///   content-part events. `response.reasoning_summary_part.added` / `.done` also carry a `part`
///   object, but that part belongs to the **reasoning item's `summary` array** (type
///   `summary_text`), not to any message's `content`. Collecting it here conflated the two and
///   produced a false failure; the assertion itself is unchanged and still fails if a message's
///   content ever contains `input_text`, `reasoning` or `summary_text`.
fn responses_message_part_types(frames: &[(String, Value)]) -> Vec<String> {
    let mut out = Vec::new();
    for (name, data) in frames {
        // Terminal/aggregate responses carry `response.output`.
        if let Some(items) = data.pointer("/response/output").and_then(Value::as_array) {
            for item in items {
                if item.get("type").and_then(Value::as_str) == Some("message") {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        out.extend(
                            parts
                                .iter()
                                .filter_map(|p| p.get("type").and_then(Value::as_str))
                                .map(str::to_string),
                        );
                    }
                }
            }
        }
        // Streaming item events carry `item` (message items only).
        if let Some(item) = data.pointer("/item") {
            if item.get("type").and_then(Value::as_str) == Some("message") {
                if let Some(parts) = item.get("content").and_then(Value::as_array) {
                    out.extend(
                        parts
                            .iter()
                            .filter_map(|p| p.get("type").and_then(Value::as_str))
                            .map(str::to_string),
                    );
                }
            }
        }
        // Streaming content-part events carry `part` — scoped to the MESSAGE item's own
        // `response.content_part.*` frames, never the reasoning summary part frames.
        if name.starts_with("response.content_part.") {
            if let Some(part) = data.pointer("/part") {
                if let Some(t) = part.get("type").and_then(Value::as_str) {
                    out.push(t.to_string());
                }
            }
        }
    }
    out
}

// ===========================================================================
// (a) truncated / aborted upstream
// ===========================================================================

/// A-SS-10 / A-SS-12 / A-SS-15 — the upstream ends cleanly with **no `finish_reason`**: the gateway
/// must still close the Anthropic conversation with a well-formed terminal set, and must not put
/// zero-length text deltas or a `stop_sequence`-less `message_delta` on the wire.
#[tokio::test]
async fn a_ss_a1_clean_eof_without_finish_reason_still_terminates() {
    let base = spawn_axum_upstream_and_gateway(eof_without_finish_reason_router()).await;
    let resp = post_messages_stream(&base, "say ping").await;
    assert_eq!(resp.status(), 200);
    let raw = read_stream_body(resp.text().await, "A-SS-12");

    let mut frames = parse_sse(&raw);
    frames.retain(|(name, _)| name != "ping");

    assert_eq!(
        frames.last().expect("stream must not be empty").0,
        "message_stop",
        "A-SS-12: message_stop must be the terminal event even when the upstream never sent a \
         finish_reason; raw body: {raw}"
    );

    let message_delta = frames
        .iter()
        .find(|(n, _)| n == "message_delta")
        .map(|(_, d)| d)
        .unwrap_or_else(|| panic!("A-SS-12: no message_delta before the terminal stop; raw body: {raw}"));
    assert_path_eq(message_delta, "/delta/stop_sequence", Value::Null, "A-SS-10");

    let empty_deltas = frames
        .iter()
        .filter(|(n, d)| {
            n == "content_block_delta"
                && d.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta")
                && d.pointer("/delta/text").and_then(Value::as_str) == Some("")
        })
        .count();
    assert_eq!(
        empty_deltas, 0,
        "A-SS-15: zero-length text_delta frames MUST NOT be emitted on the truncated path; raw body: {raw}"
    );
}

/// A-SS-18 (contract v2) — the upstream **aborts mid-stream** (socket closed without the
/// terminating chunk): the gateway must still emit exactly one terminal event, `message_stop` or
/// `error`, and must never leave the client body truncated with no terminal event. No invalid /
/// cross-protocol content-block type may appear on this path.
#[tokio::test]
async fn a_ss_a2_aborted_upstream_still_emits_terminal_set() {
    let parts = vec![
        chat_chunk("chatcmpl-abort", "{\"role\":\"assistant\"}", None, None).into_bytes(),
        chat_chunk("chatcmpl-abort", "{\"content\":\"pi\"}", None, None).into_bytes(),
    ];
    let upstream = spawn_raw_chunked_upstream(parts, false).await;
    let base = spawn_gateway_for(&upstream).await;

    let resp = post_messages_stream(&base, "say ping").await;
    assert_eq!(resp.status(), 200);
    let raw = read_stream_body(resp.text().await, "A-SS-18");

    let mut frames = parse_sse(&raw);
    frames.retain(|(name, _)| name != "ping");

    assert_terminal_event(&frames, &["message_stop", "error"], "/error/message", "A-SS-18");

    let block_types = anthropic_block_types(&frames);
    assert!(
        block_types
            .iter()
            .all(|t| OFFICIAL_ANTHROPIC_BLOCK_TYPES.contains(&t.as_str())),
        "A-SS-18: no invalid / cross-protocol content-block type may appear on the abort path; \
         allowed {OFFICIAL_ANTHROPIC_BLOCK_TYPES:?}; got {block_types:?}; raw body: {raw}"
    );
}

/// R-SS-12 / R-SS-13 — the upstream ends cleanly with **no `finish_reason`**: the Responses stream
/// must still terminate at `response.completed` with an `output_text` content part.
#[tokio::test]
async fn r_ss_a1_clean_eof_without_finish_reason_still_terminates() {
    let base = spawn_axum_upstream_and_gateway(eof_without_finish_reason_router()).await;
    let resp = post_responses_stream(&base, "say ping").await;
    assert_eq!(resp.status(), 200);
    let raw = read_stream_body(resp.text().await, "R-SS-12");

    let frames = parse_sse(&raw);
    let last = frames.last().expect("stream must not be empty");
    assert_eq!(
        last.0, "response.completed",
        "R-SS-12: response.completed must be the terminal event even without an upstream \
         finish_reason; raw body: {raw}"
    );
    assert_path_eq(
        &last.1,
        "/response/status",
        Value::String("completed".into()),
        "R-SS-12",
    );

    // The terminal aggregate must actually contain the assistant message (never assert vacuously).
    let output = last
        .1
        .pointer("/response/output")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "R-SS-12: terminal `/response/output` must be an array; terminal event: {}",
                last.1
            )
        });
    let message = output
        .iter()
        .find(|it| it.get("type").and_then(Value::as_str) == Some("message"))
        .unwrap_or_else(|| {
            panic!(
                "R-SS-12: terminal output must contain the assistant message item; terminal event: {}",
                last.1
            )
        });
    assert_path_eq(
        message,
        "/content/0/type",
        Value::String("output_text".into()),
        "R-SS-13",
    );
    assert_path_eq(
        message,
        "/content/0/text",
        Value::String(ANSWER.into()),
        "R-SS-13",
    );

    let part_types = responses_message_part_types(&frames);
    assert!(
        !part_types.is_empty(),
        "R-SS-13: the terminal aggregate must carry at least one content part (guards against a vacuous pass); raw body: {raw}"
    );
    assert!(
        !part_types.iter().any(|t| t == "input_text" || t == "reasoning"),
        "R-SS-13: a message content part must never be `input_text` or `reasoning`; got {part_types:?}; raw body: {raw}"
    );
}

/// R-SS-22 (contract v2) — the upstream **aborts mid-stream**: the Responses stream must still emit
/// exactly one terminal event, `response.completed` or `response.failed`, and must never leave the
/// client body truncated with no terminal event. No invalid / non-official content-part type may
/// appear on this path.
#[tokio::test]
async fn r_ss_a2_aborted_upstream_still_emits_terminal_event() {
    let parts = vec![
        chat_chunk("chatcmpl-abort", "{\"role\":\"assistant\"}", None, None).into_bytes(),
        chat_chunk("chatcmpl-abort", "{\"content\":\"pi\"}", None, None).into_bytes(),
    ];
    let upstream = spawn_raw_chunked_upstream(parts, false).await;
    let base = spawn_gateway_for(&upstream).await;

    let resp = post_responses_stream(&base, "say ping").await;
    assert_eq!(resp.status(), 200);
    let raw = read_stream_body(resp.text().await, "R-SS-22");

    let frames = parse_sse(&raw);
    assert_terminal_event(
        &frames,
        &["response.completed", "response.failed"],
        "/response/error/message",
        "R-SS-22",
    );

    let part_types = responses_message_part_types(&frames);
    assert!(
        part_types
            .iter()
            .all(|t| t == "output_text" || t == "refusal"),
        "R-SS-13: no invalid / non-official content-part type may appear on the abort path; \
         got {part_types:?}; raw body: {raw}"
    );
}

// ===========================================================================
// (a3) multi-choice upstream chunks — terminal event emitted exactly once, last
// ===========================================================================

/// The FIRST upstream chunk carries **three** choices: two `finish_reason` choices (which a
/// mid-loop `done` guard must collapse into a single terminal event) followed by a choice that
/// still carries content (which must never be emitted after the terminal). A second chunk keeps
/// carrying content after the turn already finished.
fn multi_choice_finish_parts() -> Vec<Vec<u8>> {
    let finish_chunk = chat_chunk_multi(
        "chatcmpl-multi",
        "[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":\"stop\"},\
          {\"index\":1,\"delta\":{},\"finish_reason\":\"stop\"},\
          {\"index\":2,\"delta\":{\"content\":\"late\"},\"finish_reason\":null}]",
        ",\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":3,\"total_tokens\":8}",
    );
    let late_content = chat_chunk_multi(
        "chatcmpl-multi",
        "[{\"index\":0,\"delta\":{\"content\":\"later\"},\"finish_reason\":null}]",
        "",
    );
    vec![
        finish_chunk.into_bytes(),
        late_content.into_bytes(),
        b"data: [DONE]\n\n".to_vec(),
    ]
}

/// A-SS-12 — a single upstream chunk carrying **more than one choice**, both with
/// `finish_reason:"stop"`, must yield **exactly one** terminal `message_stop`, it must be the LAST
/// frame, and nothing may follow it.
///
/// Regression: the per-choice loop in `crates/ponyllm-protocol/src/translator/stream.rs` did not
/// re-check `self.done` mid-chunk, so the second `finish_reason` choice emitted a duplicate
/// terminal set (`message_delta`, `message_stop` twice).
#[tokio::test]
async fn a_ss_a3_multi_choice_chunk_emits_exactly_one_terminal() {
    let upstream = spawn_raw_chunked_upstream(multi_choice_finish_parts(), true).await;
    let base = spawn_gateway_for(&upstream).await;

    let resp = post_messages_stream(&base, "say ping").await;
    assert_eq!(resp.status(), 200);
    let raw = read_stream_body(resp.text().await, "A-SS-12");

    let mut frames = parse_sse(&raw);
    frames.retain(|(name, _)| name != "ping");

    let stop_positions: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, (n, _))| n == "message_stop")
        .map(|(i, _)| i)
        .collect();
    let terminal = stop_positions.first().copied();
    let after_terminal: Vec<String> = match terminal {
        Some(t) => frames[t + 1..].iter().map(|(n, _)| n.clone()).collect(),
        None => Vec::new(),
    };
    let content_after: Vec<String> = after_terminal
        .iter()
        .filter(|n| n.as_str() == "content_block_delta")
        .cloned()
        .collect();

    // (a) exactly one terminal AND (b) nothing content-bearing after it — evaluated together so
    // the red phase reports BOTH halves in a single failure instead of short-circuiting on (a).
    assert!(
        stop_positions.len() == 1 && content_after.is_empty(),
        "A-SS-12: a single turn must emit exactly ONE terminal `message_stop` as the LAST frame, \
         with no content-bearing event after it. Observed: {} terminal(s) at {stop_positions:?}; \
         frames after the first terminal: {after_terminal:?}; content-bearing after it: \
         {content_after:?}. Raw body: {raw}",
        stop_positions.len()
    );

    assert_eq!(
        terminal,
        Some(frames.len() - 1),
        "A-SS-12: `message_stop` must be the LAST frame; raw body: {raw}"
    );
    assert!(
        after_terminal.is_empty(),
        "A-SS-12: no frame at all may follow the terminal `message_stop`; got {after_terminal:?}; \
         raw body: {raw}"
    );
}

/// R-SS-12 — the same multi-choice chunk shape on `/v1/responses` must yield **exactly one**
/// `response.completed`, it must be the last frame, and no `output_text.delta` may follow it.
///
/// Regression: the per-choice loop in `crates/ponyllm-protocol/src/translator/responses_stream.rs`
/// did not re-check its terminal state mid-chunk, so the second `finish_reason` choice emitted a
/// second `response.completed`.
#[tokio::test]
async fn r_ss_a3_multi_choice_chunk_emits_exactly_one_terminal() {
    let upstream = spawn_raw_chunked_upstream(multi_choice_finish_parts(), true).await;
    let base = spawn_gateway_for(&upstream).await;

    let resp = post_responses_stream(&base, "say ping").await;
    assert_eq!(resp.status(), 200);
    let raw = read_stream_body(resp.text().await, "R-SS-12");

    let frames = parse_sse(&raw);

    let completed_positions: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, (n, _))| n == "response.completed")
        .map(|(i, _)| i)
        .collect();
    let terminal = completed_positions.first().copied();
    let after_terminal: Vec<String> = match terminal {
        Some(t) => frames[t + 1..].iter().map(|(n, _)| n.clone()).collect(),
        None => Vec::new(),
    };
    let content_after: Vec<String> = after_terminal
        .iter()
        .filter(|n| n.as_str() == "response.output_text.delta")
        .cloned()
        .collect();

    // (a) exactly one terminal AND (b) nothing content-bearing after it — evaluated together so
    // the red phase reports BOTH halves in a single failure instead of short-circuiting on (a).
    assert!(
        completed_positions.len() == 1 && content_after.is_empty(),
        "R-SS-12: a single turn must emit exactly ONE terminal `response.completed` as the LAST \
         frame, with no content-bearing event after it. Observed: {} terminal(s) at \
         {completed_positions:?}; frames after the first terminal: {after_terminal:?}; \
         content-bearing after it: {content_after:?}. Raw body: {raw}",
        completed_positions.len()
    );

    assert_eq!(
        terminal,
        Some(frames.len() - 1),
        "R-SS-12: `response.completed` must be the LAST frame; raw body: {raw}"
    );
    assert!(
        after_terminal.is_empty(),
        "R-SS-12: no frame at all may follow the terminal `response.completed`; got \
         {after_terminal:?}; raw body: {raw}"
    );
}

// ===========================================================================
// (b) oversized / malformed upstream payloads
// ===========================================================================

/// R-SS-12 / R-SS-13 — an **80 KiB reasoning delta** (above the gateway's 64 KiB SSE frame cap)
/// must not produce malformed JSON, must not panic, and must not leak a non-official content-part
/// type into the client stream.
#[tokio::test]
async fn r_ss_b1_oversized_reasoning_frame_no_malformed_json_no_non_official_part() {
    let huge = "R".repeat(80 * 1024);
    let oversized = chat_chunk(
        "chatcmpl-big",
        &format!("{{\"reasoning_content\":\"{huge}\"}}"),
        None,
        None,
    )
    .into_bytes();
    let answer = chat_chunk("chatcmpl-big", "{\"content\":\"ping\"}", None, None).into_bytes();
    let finish = chat_chunk(
        "chatcmpl-big",
        "{}",
        Some("stop"),
        Some("{\"prompt_tokens\":42,\"completion_tokens\":34,\"total_tokens\":76}"),
    )
    .into_bytes();

    let upstream = spawn_raw_chunked_upstream(vec![oversized, answer, finish], true).await;
    let base = spawn_gateway_for(&upstream).await;

    let resp = post_responses_stream(&base, "say ping").await;
    assert_eq!(resp.status(), 200);
    let raw = read_stream_body(resp.text().await, "R-SS-12");

    // `parse_sse` panics on any `data:` line that is not valid JSON.
    let frames = parse_sse(&raw);
    let last = frames.last().expect("stream must not be empty");
    assert_eq!(
        last.0, "response.completed",
        "R-SS-12: an oversized upstream frame must not break the terminal event; raw body: {raw}"
    );

    let part_types = responses_message_part_types(&frames);
    assert!(
        !part_types.is_empty(),
        "R-SS-13: the message item must carry at least one content part (guards against a vacuous pass); raw body: {raw}"
    );
    assert!(
        !part_types.iter().any(|t| t == "input_text" || t == "reasoning"),
        "R-SS-13: a message content part must never be `input_text` or `reasoning`; got {part_types:?}; raw body: {raw}"
    );
    assert!(
        part_types.iter().all(|t| t == "output_text" || t == "refusal"),
        "R-SS-13: message content parts must be `output_text`/`refusal` only; got {part_types:?}; raw body: {raw}"
    );
}

/// A-SS-12 / A-SS-15 — an unparseable upstream frame plus a zero-length delta must not panic, must
/// not put malformed JSON on the client wire, must not emit empty `text_delta` frames, and must
/// still terminate.
#[tokio::test]
async fn a_ss_b2_malformed_and_zero_length_frames_stay_wellformed() {
    let mut body = String::new();
    body.push_str(&chat_chunk("chatcmpl-mal", "{\"role\":\"assistant\"}", None, None));
    body.push_str("data: {this is not json at all\n\n");
    body.push_str(&chat_chunk("chatcmpl-mal", "{\"content\":\"\"}", None, None));
    body.push_str(&chat_chunk("chatcmpl-mal", "{\"content\":\"ping\"}", None, None));
    body.push_str(&chat_chunk(
        "chatcmpl-mal",
        "{}",
        Some("stop"),
        Some("{\"prompt_tokens\":42,\"completion_tokens\":34,\"total_tokens\":76}"),
    ));
    let parts = vec![body.into_bytes()];
    let upstream = spawn_raw_chunked_upstream(parts, true).await;
    let base = spawn_gateway_for(&upstream).await;

    let resp = post_messages_stream(&base, "say ping").await;
    assert_eq!(resp.status(), 200);
    let raw = read_stream_body(resp.text().await, "A-SS-12");

    let mut frames = parse_sse(&raw);
    frames.retain(|(name, _)| name != "ping");

    let empty_deltas = frames
        .iter()
        .filter(|(n, d)| {
            n == "content_block_delta"
                && d.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta")
                && d.pointer("/delta/text").and_then(Value::as_str) == Some("")
        })
        .count();
    assert_eq!(
        empty_deltas, 0,
        "A-SS-15: zero-length text_delta frames MUST NOT be emitted; raw body: {raw}"
    );
    assert_eq!(
        frames.last().expect("stream must not be empty").0,
        "message_stop",
        "A-SS-12: a malformed upstream frame must not prevent the terminal message_stop; raw body: {raw}"
    );
}

/// R-SS-13 — a multi-byte UTF-8 character split **across two TCP writes** must be reassembled
/// byte-exactly (no mojibake) and must not corrupt the frame into malformed JSON or a non-official
/// content-part type.
///
/// The split is structural: the two halves are written as two HTTP chunked-encoding chunks, so the
/// HTTP client decodes them as two distinct body chunks regardless of timing.
#[tokio::test]
async fn r_ss_b3_utf8_multibyte_split_across_tcp_writes_round_trips() {
    let expected = "中文";
    let answer_chunk = chat_chunk(
        "chatcmpl-split",
        &format!("{{\"content\":\"{expected}\"}}"),
        None,
        None,
    );
    // Split inside the 3-byte UTF-8 encoding of the first character: "中" is E4 B8 AD.
    let split_at = answer_chunk
        .find('中')
        .expect("fixture must contain the multi-byte character")
        + 1;
    assert_eq!(
        &answer_chunk.as_bytes()[split_at - 1..split_at + 1],
        &[0xE4, 0xB8],
        "fixture must split the 3-byte UTF-8 sequence after its first byte"
    );

    let first = answer_chunk.as_bytes()[..split_at].to_vec();
    let second = answer_chunk.as_bytes()[split_at..].to_vec();
    let finish = chat_chunk(
        "chatcmpl-split",
        "{}",
        Some("stop"),
        Some("{\"prompt_tokens\":42,\"completion_tokens\":34,\"total_tokens\":76}"),
    )
    .into_bytes();
    let role = chat_chunk("chatcmpl-split", "{\"role\":\"assistant\"}", None, None).into_bytes();

    let upstream = spawn_raw_chunked_upstream(vec![role, first, second, finish], true).await;
    let base = spawn_gateway_for(&upstream).await;

    let resp = post_responses_stream(&base, "say 中文").await;
    assert_eq!(resp.status(), 200);
    let raw = read_stream_body(resp.text().await, "R-SS-13");

    // `parse_sse` panics on any `data:` line that is not valid JSON.
    let frames = parse_sse(&raw);

    // The multi-byte answer must survive byte-exactly in the terminal aggregate.
    let last = frames.last().expect("stream must not be empty");
    let output = last
        .1
        .pointer("/response/output")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("R-SS-13: terminal `/response/output` must be an array: {}", last.1));
    let message = output
        .iter()
        .find(|it| it.get("type").and_then(Value::as_str) == Some("message"))
        .unwrap_or_else(|| panic!("R-SS-13: terminal output must contain a message item: {}", last.1));
    // Byte-exact reassembly of the split multi-byte character is asserted first, so a corruption
    // failure is distinguishable from the conformance failure that follows.
    assert_path_eq(
        message,
        "/content/0/text",
        Value::String(expected.into()),
        "R-SS-13: the multi-byte text must round-trip byte-exactly across the split write",
    );
    assert_path_eq(
        message,
        "/content/0/type",
        Value::String("output_text".into()),
        "R-SS-13",
    );
}

// ===========================================================================
// (c) concurrency — 8 + 8 against one gateway instance
// ===========================================================================

const CONCURRENCY: usize = 8;

/// Upstream that echoes the request's own marker back as the assistant answer, and gives each
/// request a distinct upstream chunk id, so that any shared id observed downstream is the
/// gateway's own cross-request bleed rather than the mock's.
fn echoing_upstream_router() -> Router {
    Router::new().route(
        "/v1/chat/completions",
        post(|Json(req): Json<Value>| async move {
            let raw = serde_json::to_string(&req).unwrap_or_default();
            let marker = (0..CONCURRENCY)
                .map(|i| format!("MARKER-{i}"))
                .find(|m| raw.contains(m.as_str()))
                .unwrap_or_else(|| "MARKER-none".to_string());
            let body = format!(
                "{}{}{}data: [DONE]\n\n",
                chat_chunk(
                    &format!("chatcmpl-{marker}"),
                    "{\"role\":\"assistant\"}",
                    None,
                    None
                ),
                chat_chunk(
                    &format!("chatcmpl-{marker}"),
                    &format!("{{\"content\":\"{marker}\"}}"),
                    None,
                    None
                ),
                chat_chunk(
                    &format!("chatcmpl-{marker}"),
                    "{}",
                    Some("stop"),
                    Some("{\"prompt_tokens\":5,\"completion_tokens\":3,\"total_tokens\":8}")
                ),
            );
            sse_response(body).into_response()
        }),
    )
}

/// (c) Exactly 8 simultaneous `/v1/responses` + 8 simultaneous `/v1/messages` streams against one
/// gateway instance, joined all: no cross-request text bleed, no id bleed, and no
/// `sequence_number` bleed between concurrent Responses streams.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c_concurrency_8_responses_and_8_messages_no_cross_request_bleed() {
    let base = spawn_axum_upstream_and_gateway(echoing_upstream_router()).await;
    let client = Arc::new(reqwest::Client::new());

    // --- join-all: launch all 16 requests before awaiting any of them ---
    let mut response_tasks = Vec::new();
    for i in 0..CONCURRENCY {
        let base = base.clone();
        let client = client.clone();
        response_tasks.push(tokio::spawn(async move {
            let resp = client
                .post(format!("{base}/v1/responses"))
                .json(&json!({
                    "model": ROUTED_MODEL,
                    "input": format!("MARKER-{i}"),
                    "stream": true
                }))
                .send()
                .await
                .expect("gateway must accept the request");
            assert_eq!(resp.status(), 200, "responses request {i} must be 200");
            (
                i,
                resp.text()
                    .await
                    .expect("responses stream body must be readable"),
            )
        }));
    }
    let mut message_tasks = Vec::new();
    for i in 0..CONCURRENCY {
        let base = base.clone();
        let client = client.clone();
        message_tasks.push(tokio::spawn(async move {
            let resp = client
                .post(format!("{base}/v1/messages"))
                .header("anthropic-version", "2023-06-01")
                .json(&json!({
                    "model": ROUTED_MODEL,
                    "max_tokens": 64,
                    "stream": true,
                    "messages": [{"role": "user", "content": format!("MARKER-{i}")}]
                }))
                .send()
                .await
                .expect("gateway must accept the request");
            assert_eq!(resp.status(), 200, "messages request {i} must be 200");
            (
                i,
                resp.text()
                    .await
                    .expect("messages stream body must be readable"),
            )
        }));
    }

    let mut response_bodies = Vec::new();
    for task in response_tasks {
        response_bodies.push(task.await.expect("responses task must not panic"));
    }
    let mut message_bodies = Vec::new();
    for task in message_tasks {
        message_bodies.push(task.await.expect("messages task must not panic"));
    }
    assert_eq!(response_bodies.len(), CONCURRENCY);
    assert_eq!(message_bodies.len(), CONCURRENCY);

    // --- /v1/responses isolation ---
    let mut response_ids: Vec<String> = Vec::new();
    for (i, raw) in &response_bodies {
        let frames = parse_sse(raw);
        let created = frames
            .iter()
            .find(|(n, _)| n == "response.created")
            .map(|(_, d)| d)
            .unwrap_or_else(|| panic!("R-SS-01: request {i} has no response.created; body: {raw}"));

        let id = created
            .pointer("/response/id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("R-SS-19: request {i} has no response id; body: {raw}"))
            .to_string();
        assert_string_matching(created, "/response/id", "resp_", 8, "R-SS-19");

        // R-SS-04 — each concurrent stream owns its own strictly-increasing sequence.
        let numbers: Vec<i64> = frames
            .iter()
            .map(|(name, d)| {
                d.pointer("/sequence_number")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| {
                        panic!("R-SS-04: request {i} frame `{name}` has no integer `sequence_number`: {d}")
                    })
            })
            .collect();
        assert!(
            numbers.windows(2).all(|w| w[1] > w[0]),
            "R-SS-04: request {i} sequence_number must strictly increase; got {numbers:?}"
        );

        let terminal = frames
            .last()
            .unwrap_or_else(|| panic!("R-SS-12: request {i} stream is empty"));
        assert_eq!(
            terminal.0, "response.completed",
            "R-SS-12: request {i} must terminate at response.completed; body: {raw}"
        );

        // No cross-request text bleed: exactly this request's own marker.
        let markers: Vec<String> = (0..CONCURRENCY)
            .map(|m| format!("MARKER-{m}"))
            .filter(|m| raw.contains(m.as_str()))
            .collect();
        assert_eq!(
            markers,
            vec![format!("MARKER-{i}")],
            "R-SS-19: responses stream {i} must carry only its own marker (cross-request bleed); body: {raw}"
        );

        response_ids.push(id);
    }
    let mut sorted_response_ids = response_ids.clone();
    sorted_response_ids.sort();
    sorted_response_ids.dedup();
    assert_eq!(
        sorted_response_ids.len(),
        CONCURRENCY,
        "R-SS-19: the {CONCURRENCY} concurrent responses streams must have pairwise-distinct ids; got {response_ids:?}"
    );

    // --- /v1/messages isolation ---
    let mut message_ids: Vec<String> = Vec::new();
    for (i, raw) in &message_bodies {
        let mut frames = parse_sse(raw);
        frames.retain(|(n, _)| n != "ping");

        let start = frames
            .iter()
            .find(|(n, _)| n == "message_start")
            .map(|(_, d)| d)
            .unwrap_or_else(|| panic!("A-SS-01: request {i} has no message_start; body: {raw}"));
        assert_string_matching(start, "/message/id", "msg_", 1, "A-SS-06");
        message_ids.push(
            start
                .pointer("/message/id")
                .and_then(Value::as_str)
                .expect("A-SS-06: message id must be a string")
                .to_string(),
        );

        let markers: Vec<String> = (0..CONCURRENCY)
            .map(|m| format!("MARKER-{m}"))
            .filter(|m| raw.contains(m.as_str()))
            .collect();
        assert_eq!(
            markers,
            vec![format!("MARKER-{i}")],
            "A-SS-06: messages stream {i} must carry only its own marker (cross-request bleed); body: {raw}"
        );

        assert_eq!(
            frames.last().expect("stream must not be empty").0,
            "message_stop",
            "A-SS-12: request {i} must terminate at message_stop; body: {raw}"
        );
    }
    let mut sorted_message_ids = message_ids.clone();
    sorted_message_ids.sort();
    sorted_message_ids.dedup();
    assert_eq!(
        sorted_message_ids.len(),
        CONCURRENCY,
        "A-SS-06: the {CONCURRENCY} concurrent messages streams must have pairwise-distinct ids; got {message_ids:?}"
    );
}
