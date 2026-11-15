//! Verifier-owned **independent** acceptance suite for the native Gemini upstream protocol.
//!
//! Source of truth (deliberately *not* the implementer's fixtures):
//!
//! * `/.agents/notes/proposed/feature/2026-11-15-gemini-native-upstream-protocol.md`
//!   (Acceptance criteria §3/§6: three downstream protocols, native URL shapes,
//!   `x-goog-api-key` only, error classification).
//! * Gemini Developer API REST semantics:
//!   `POST {base}/models/{model}:generateContent`,
//!   `POST {base}/models/{model}:streamGenerateContent?alt=sse`,
//!   request body `{contents, systemInstruction, generationConfig, tools}` (model name lives in
//!   the **path**, never in the body),
//!   response body `{candidates:[{content:{parts:[…]}, finishReason}], usageMetadata}` where a
//!   part is `{text}` / `{text, thought:true}` / `{functionCall:{id,name,args}}`.
//! * Downstream wire shapes asserted here are the official OpenAI Chat Completions, Anthropic
//!   Messages and OpenAI Responses shapes (field names taken from those public APIs), not the
//!   gateway's internal translator structs.
//!
//! Everything is asserted over the wire through the real `create_app` router in front of a local
//! axum mock Gemini upstream. No sleeps, no `#[ignore]`, no swallowed errors: a broken gateway
//! makes these tests exit non-zero.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use ponyllm_config::AuthMode;
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::{json, Value};

const PROVIDER: &str = "gemini-direct";
const MODEL: &str = "gemini-acceptance-model";
const API_KEY: &str = "AQ.verifyonly0000fakekey";
const PROMPT: &str = "What's the weather in Paris?";
const SYSTEM: &str = "You are a terse weather bot.";

/// One `thought: true` part (Gemini's reasoning channel).
const THOUGHT: &str = "The user asks about Paris weather; I should call get_weather.";
/// One ordinary text part (the visible answer).
const ANSWER: &str = "Checking the weather in Paris now.";
/// The upstream-provided function-call id — must survive to every downstream protocol.
const TOOL_ID: &str = "call_gem7x42";
const TOOL_NAME: &str = "get_weather";

// `usageMetadata` is internally consistent: total = prompt + candidates + thoughts.
const PROMPT_TOKENS: u32 = 42;
const CANDIDATE_TOKENS: u32 = 17;
const THOUGHT_TOKENS: u32 = 23;
const CACHED_TOKENS: u32 = 8;
const TOTAL_TOKENS: u32 = 82;
/// Gemini bills thoughts as output, so the OpenAI/Anthropic completion count is 17 + 23.
const COMPLETION_TOKENS: u32 = CANDIDATE_TOKENS + THOUGHT_TOKENS;

// ---------------------------------------------------------------------------
// mock Gemini upstream
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Real Gemini shapes: thought part, text part, functionCall part, then a separate
    /// `finishReason: STOP` frame carrying `usageMetadata`.
    Full,
    /// Gemini answers 400 `API_KEY_INVALID` for a bad key.
    BadApiKey,
    /// Gemini answers 429 `RESOURCE_EXHAUSTED` for a per-model free-tier window.
    QuotaExhausted,
}

#[derive(Clone, Debug)]
struct Captured {
    method: String,
    path: String,
    query: Option<String>,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone)]
struct MockState {
    captured: Arc<Mutex<Vec<Captured>>>,
    /// Makes `responseId` unique per upstream request so cross-request bleed is detectable.
    request_seq: Arc<AtomicUsize>,
    mode: Mode,
}

impl MockState {
    fn new(mode: Mode) -> Self {
        Self {
            captured: Arc::new(Mutex::new(Vec::new())),
            request_seq: Arc::new(AtomicUsize::new(0)),
            mode,
        }
    }

    fn captured(&self) -> Vec<Captured> {
        self.captured.lock().unwrap().clone()
    }
}

fn usage_metadata() -> Value {
    json!({
        "promptTokenCount": PROMPT_TOKENS,
        "candidatesTokenCount": CANDIDATE_TOKENS,
        "thoughtsTokenCount": THOUGHT_TOKENS,
        "cachedContentTokenCount": CACHED_TOKENS,
        "totalTokenCount": TOTAL_TOKENS
    })
}

/// Native `generateContent` body: thought part + text part + functionCall part, `STOP`.
///
/// Gemini reports `STOP` (never `FUNCTION_CALL`) after a function call, which is exactly why a
/// turn carrying `functionCall` must reach the client as a tool-call finish.
fn native_response(response_id: &str) -> Value {
    json!({
        "candidates": [{
            "content": {
                "parts": [
                    {"text": THOUGHT, "thought": true},
                    {"text": ANSWER},
                    {
                        "functionCall": {
                            "id": TOOL_ID,
                            "name": TOOL_NAME,
                            "args": {"location": "Paris", "unit": "celsius"}
                        },
                        "thoughtSignature": "sig-verifier-fixture"
                    }
                ],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": usage_metadata(),
        "modelVersion": MODEL,
        "responseId": response_id
    })
}

/// Native `:streamGenerateContent?alt=sse` body. The `functionCall` and the `finishReason: STOP`
/// terminator arrive in **separate frames** (this is the real upstream shape).
fn native_sse_body(response_id: &str) -> String {
    let frames = [
        json!({
            "candidates": [{
                "content": {"parts": [{"text": THOUGHT, "thought": true}], "role": "model"},
                "index": 0
            }],
            "modelVersion": MODEL,
            "responseId": response_id
        }),
        json!({
            "candidates": [{
                "content": {"parts": [{"text": ANSWER}], "role": "model"},
                "index": 0
            }],
            "modelVersion": MODEL,
            "responseId": response_id
        }),
        json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "id": TOOL_ID,
                            "name": TOOL_NAME,
                            "args": {"location": "Paris", "unit": "celsius"}
                        }
                    }],
                    "role": "model"
                },
                "index": 0
            }],
            "modelVersion": MODEL,
            "responseId": response_id
        }),
        json!({
            "candidates": [{
                "content": {"parts": [], "role": "model"},
                "finishReason": "STOP",
                "index": 0
            }],
            "usageMetadata": usage_metadata(),
            "modelVersion": MODEL,
            "responseId": response_id
        }),
    ];
    let mut out = String::new();
    for frame in frames {
        out.push_str(&format!("data: {frame}\n\n"));
    }
    out
}

fn json_upstream(status: StatusCode, body: Value) -> Response {
    (status, axum::Json(body)).into_response()
}

fn sse_upstream(body: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from(body))
        .expect("build mock SSE response")
}

async fn mock_gemini(
    State(state): State<MockState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    state.captured.lock().unwrap().push(Captured {
        method: method.to_string(),
        path: uri.path().to_string(),
        query: uri.query().map(str::to_string),
        headers: headers
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
            .collect(),
        body: parsed,
    });

    let response_id = format!(
        "gem-verifier-{}",
        state.request_seq.fetch_add(1, Ordering::SeqCst) + 1
    );

    match state.mode {
        Mode::BadApiKey => json_upstream(
            StatusCode::BAD_REQUEST,
            json!({
                "error": {
                    "code": 400,
                    "message": "API key not valid. Please pass a valid API key.",
                    "status": "API_KEY_INVALID",
                    "details": [{
                        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                        "reason": "API_KEY_INVALID",
                        "domain": "googleapis.com",
                        "metadata": {"service": "generativelanguage.googleapis.com"}
                    }]
                }
            }),
        ),
        Mode::QuotaExhausted => json_upstream(
            StatusCode::TOO_MANY_REQUESTS,
            json!({
                "error": {
                    "code": 429,
                    "message": "You exceeded your current quota, please check your plan and billing details.",
                    "status": "RESOURCE_EXHAUSTED"
                }
            }),
        ),
        Mode::Full => {
            if uri.path().contains(":streamGenerateContent") {
                sse_upstream(native_sse_body(&response_id))
            } else {
                json_upstream(StatusCode::OK, native_response(&response_id))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// gateway harness
// ---------------------------------------------------------------------------

struct Harness {
    base: String,
    upstream: MockState,
}

impl Harness {
    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("build reqwest client")
    }

    async fn post(&self, path: &str, body: Value, anthropic: bool) -> (StatusCode, String) {
        let mut req = Self::client()
            .post(format!("{}{}", self.base, path))
            .header("content-type", "application/json")
            .json(&body);
        if anthropic {
            req = req.header("anthropic-version", "2023-06-01");
        }
        let resp = req
            .send()
            .await
            .unwrap_or_else(|e| panic!("POST {path} must reach the gateway: {e}"));
        let status = resp.status();
        let text = resp
            .text()
            .await
            .unwrap_or_else(|e| panic!("reading the body of POST {path} failed: {e}"));
        (status, text)
    }

    async fn post_json(&self, path: &str, body: Value) -> (StatusCode, Value) {
        let (status, text) = self.post(path, body, path == "/v1/messages").await;
        let parsed = serde_json::from_str::<Value>(&text).unwrap_or_else(|e| {
            panic!("{path}: expected a JSON body, got {e}; raw body: {text}");
        });
        (status, parsed)
    }
}

async fn spawn_harness(mode: Mode) -> Harness {
    let upstream = MockState::new(mode);
    let router = Router::new()
        .fallback(any(mock_gemini))
        .with_state(upstream.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock upstream");
    let addr = listener.local_addr().expect("mock upstream addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve mock upstream");
    });

    let pool = Arc::new(KeyPool::new(PROVIDER, RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("gemini-key-1", API_KEY, 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = AuthMode::Open;
    config.providers.insert(
        PROVIDER.to_string(),
        ProviderConfig {
            base_url: format!("http://{addr}/v1beta"),
            default_model: MODEL.to_string(),
            strategy: "round_robin".to_string(),
            models: vec![MODEL.to_string()],
            model_specs: vec![ModelSpec {
                name: MODEL.to_string(),
                ..Default::default()
            }],
            default_protocol: Some(UpstreamProtocol::Gemini),
            ..Default::default()
        },
    );

    let state = Arc::new(AppState::new(config));
    state.register_pool(PROVIDER, pool);

    let app = create_app(state);
    let gw_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind gateway");
    let gw_addr = gw_listener.local_addr().expect("gateway addr");
    tokio::spawn(async move {
        axum::serve(gw_listener, app).await.expect("serve gateway");
    });

    Harness {
        base: format!("http://{gw_addr}"),
        upstream,
    }
}

// ---------------------------------------------------------------------------
// SSE helpers
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Frame {
    event: Option<String>,
    data: Value,
    is_done: bool,
}

/// Parse an SSE body into frames. Any `data:` payload that is not valid JSON is a hard failure —
/// this is the "no malformed JSON on the client wire" gate for every streaming case.
#[track_caller]
fn parse_sse(body: &str, case: &str) -> Vec<Frame> {
    let mut frames = Vec::new();
    for block in body.split("\n\n") {
        let block = block.trim_matches(|c| c == '\n' || c == '\r');
        if block.is_empty() {
            continue;
        }
        let mut event: Option<String> = None;
        let mut data: Option<String> = None;
        for line in block.lines() {
            let line = line.trim_end_matches('\r');
            if line.starts_with(':') {
                continue; // heartbeat comment
            }
            if let Some(rest) = line.strip_prefix("event:") {
                event = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                data = Some(rest.trim().to_string());
            }
        }
        let Some(data) = data else {
            panic!("{case}: SSE block without a `data:` line: {block:?}");
        };
        if data == "[DONE]" {
            frames.push(Frame {
                event,
                data: Value::Null,
                is_done: true,
            });
            continue;
        }
        let parsed: Value = serde_json::from_str(&data).unwrap_or_else(|e| {
            panic!("{case}: client-facing SSE payload is not valid JSON ({e}): {data}")
        });
        frames.push(Frame {
            event,
            data: parsed,
            is_done: false,
        });
    }
    frames
}

fn tool_args() -> Value {
    json!({"location": "Paris", "unit": "celsius"})
}

fn parsed_arguments(raw: &str, case: &str) -> Value {
    serde_json::from_str::<Value>(raw)
        .unwrap_or_else(|e| panic!("{case}: tool-call arguments must be JSON text ({e}): {raw}"))
}

/// Outbound Gemini contract shared by every downstream protocol.
#[track_caller]
fn assert_native_outbound(cap: &Captured, streaming: bool, case: &str) {
    assert_eq!(cap.method, "POST", "{case}: Gemini calls are POST");

    let verb = if streaming {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    let expected_path = format!("/v1beta/models/{MODEL}:{verb}");
    assert_eq!(
        cap.path, expected_path,
        "{case}: Gemini's model name lives in the URL path (`{{base}}/models/{{model}}:{verb}`)"
    );
    if streaming {
        assert_eq!(
            cap.query.as_deref(),
            Some("alt=sse"),
            "{case}: `:streamGenerateContent` requires `alt=sse` to yield SSE instead of a JSON array"
        );
    } else {
        assert_eq!(
            cap.query, None,
            "{case}: non-streaming `:generateContent` takes no query string"
        );
    }

    assert_eq!(
        cap.header("x-goog-api-key"),
        Some(API_KEY),
        "{case}: Gemini API keys travel in `x-goog-api-key`"
    );
    for forbidden in ["authorization", "x-api-key", "anthropic-version"] {
        assert!(
            cap.header(forbidden).is_none(),
            "{case}: `{forbidden}` must never be sent to generativelanguage.googleapis.com (got {:?})",
            cap.header(forbidden)
        );
    }

    let contents = cap
        .body
        .get("contents")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{case}: native Gemini body needs a `contents` array: {}", cap.body));
    assert!(!contents.is_empty(), "{case}: `contents` must not be empty");
    assert_eq!(
        contents[0].get("role").and_then(Value::as_str),
        Some("user"),
        "{case}: first turn must be the user turn"
    );
    assert!(
        cap.body.get("model").is_none(),
        "{case}: the model is addressed by URL path, the body must not carry `model`: {}",
        cap.body
    );
}

// ===========================================================================
// A1 — /v1/chat/completions, non-streaming
// ===========================================================================

#[tokio::test]
async fn a1_chat_non_stream_surfaces_text_reasoning_tool_call_and_usage() {
    let h = spawn_harness(Mode::Full).await;
    let (status, body) = h
        .post_json(
            "/v1/chat/completions",
            json!({"model": MODEL, "messages": [{"role": "user", "content": PROMPT}]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "A1: body={body}");
    assert_eq!(body["object"], "chat.completion", "A1: body={body}");
    assert_eq!(body["model"], MODEL, "A1: the gateway echoes the requested model");

    let choice = &body["choices"][0];
    assert_eq!(choice["index"], 0);
    assert_eq!(choice["message"]["role"], "assistant");
    assert_eq!(
        choice["message"]["content"], ANSWER,
        "A1: plain Gemini text parts map to assistant content"
    );
    assert_eq!(
        choice["message"]["reasoning_content"], THOUGHT,
        "A1: `thought: true` Gemini parts map to `reasoning_content`"
    );
    assert!(
        !choice["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .contains(THOUGHT),
        "A1: thought text must not leak into the visible content"
    );

    let tool = &choice["message"]["tool_calls"][0];
    assert_eq!(tool["id"], TOOL_ID, "A1: upstream functionCall.id is preserved");
    assert_eq!(tool["type"], "function");
    assert_eq!(tool["function"]["name"], TOOL_NAME);
    assert_eq!(
        parsed_arguments(
            tool["function"]["arguments"]
                .as_str()
                .unwrap_or_else(|| panic!("A1: tool arguments must be a JSON string: {tool}")),
            "A1"
        ),
        tool_args(),
        "A1: Gemini `functionCall.args` must become the OpenAI arguments object"
    );

    assert_eq!(
        choice["finish_reason"], "tool_calls",
        "A1: Gemini reports STOP for tool turns — the client-facing finish must be `tool_calls`"
    );

    assert_eq!(body["usage"]["prompt_tokens"], PROMPT_TOKENS);
    assert_eq!(
        body["usage"]["completion_tokens"], COMPLETION_TOKENS,
        "A1: thoughts are billed as output, so completion = candidates + thoughts"
    );
    assert_eq!(body["usage"]["total_tokens"], TOTAL_TOKENS);
    assert_eq!(
        body["usage"]["completion_tokens_details"]["reasoning_tokens"], THOUGHT_TOKENS,
        "A1: `thoughtsTokenCount` must surface as `reasoning_tokens`"
    );
    assert_eq!(
        body["usage"]["prompt_tokens_details"]["cached_tokens"], CACHED_TOKENS,
        "A1: `cachedContentTokenCount` must surface as `cached_tokens`"
    );
}

// ===========================================================================
// A2 — /v1/chat/completions, streaming
// ===========================================================================

#[tokio::test]
async fn a2_chat_stream_emits_reasoning_text_tool_calls_and_done() {
    let h = spawn_harness(Mode::Full).await;
    let (status, raw) = h
        .post(
            "/v1/chat/completions",
            json!({"model": MODEL, "stream": true, "messages": [{"role": "user", "content": PROMPT}]}),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "A2: body={raw}");

    let frames = parse_sse(&raw, "A2");
    assert!(!frames.is_empty(), "A2: the stream must not be empty: {raw}");
    let last = frames.last().unwrap();
    assert!(
        last.is_done,
        "A2: an OpenAI chat stream terminates with `data: [DONE]`; last frame data={:?}",
        last.data
    );
    assert!(
        frames[..frames.len() - 1].iter().all(|f| !f.is_done),
        "A2: `[DONE]` must appear exactly once, at the end"
    );

    let chunks: Vec<&Value> = frames.iter().filter(|f| !f.is_done).map(|f| &f.data).collect();
    assert!(!chunks.is_empty(), "A2: at least one chat chunk is required");

    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_id: Option<String> = None;
    let mut tool_name: Option<String> = None;
    let mut tool_arguments: Option<String> = None;
    let mut finish_reasons: Vec<String> = Vec::new();
    let mut ids: Vec<String> = Vec::new();

    for chunk in &chunks {
        assert_eq!(chunk["object"], "chat.completion.chunk", "A2: chunk={chunk}");
        if let Some(id) = chunk["id"].as_str() {
            if !ids.iter().any(|seen| seen == id) {
                ids.push(id.to_string());
            }
        }
        for choice in chunk["choices"].as_array().cloned().unwrap_or_default() {
            if let Some(text) = choice["delta"]["content"].as_str() {
                content.push_str(text);
            }
            if let Some(text) = choice["delta"]["reasoning_content"].as_str() {
                reasoning.push_str(text);
            }
            if let Some(calls) = choice["delta"]["tool_calls"].as_array() {
                for call in calls {
                    if let Some(id) = call["id"].as_str() {
                        tool_id.get_or_insert_with(|| id.to_string());
                    }
                    if let Some(name) = call["function"]["name"].as_str() {
                        tool_name.get_or_insert_with(|| name.to_string());
                    }
                    if let Some(args) = call["function"]["arguments"].as_str() {
                        tool_arguments.get_or_insert_with(|| args.to_string());
                    }
                }
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                finish_reasons.push(reason.to_string());
            }
        }
    }

    assert_eq!(content, ANSWER, "A2: concatenated text deltas must equal the upstream text");
    assert_eq!(
        reasoning, THOUGHT,
        "A2: concatenated reasoning deltas (`reasoning_content`) must equal the upstream thought"
    );
    assert_eq!(tool_id.as_deref(), Some(TOOL_ID), "A2: the functionCall id must reach the client");
    assert_eq!(
        tool_name.as_deref(),
        Some(TOOL_NAME),
        "A2: the function name must reach the client"
    );
    assert_eq!(
        parsed_arguments(
            tool_arguments
                .as_deref()
                .unwrap_or_else(|| panic!("A2: no tool-call arguments delta was streamed")),
            "A2"
        ),
        tool_args()
    );

    assert_eq!(
        finish_reasons,
        vec!["tool_calls".to_string()],
        "A2: exactly one terminal finish_reason, and a STOP-after-functionCall frame must be \
         promoted to `tool_calls` (a bare `stop` ends the tool round early)"
    );
    assert_eq!(
        ids.len(),
        1,
        "A2: one stream must carry one stable chunk id, saw {ids:?}"
    );
}

// ===========================================================================
// A3 — /v1/messages, non-streaming
// ===========================================================================

#[tokio::test]
async fn a3_messages_non_stream_yields_thinking_text_and_tool_use_blocks() {
    let h = spawn_harness(Mode::Full).await;
    let (status, body) = h
        .post_json(
            "/v1/messages",
            json!({
                "model": MODEL,
                "max_tokens": 1024,
                "messages": [{"role": "user", "content": PROMPT}]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "A3: body={body}");
    assert_eq!(body["type"], "message");
    assert_eq!(body["role"], "assistant");

    let blocks = body["content"]
        .as_array()
        .unwrap_or_else(|| panic!("A3: Anthropic content must be an array: {body}"));
    let types: Vec<&str> = blocks
        .iter()
        .map(|b| b["type"].as_str().unwrap_or("<missing>"))
        .collect();
    assert_eq!(
        types,
        vec!["thinking", "text", "tool_use"],
        "A3: thought → thinking block, text → text block, functionCall → tool_use block; got {body}"
    );
    assert_eq!(blocks[0]["thinking"], THOUGHT);
    assert_eq!(blocks[1]["text"], ANSWER);
    assert_eq!(blocks[2]["id"], TOOL_ID, "A3: tool_use keeps the upstream functionCall.id");
    assert_eq!(blocks[2]["name"], TOOL_NAME);
    assert_eq!(
        blocks[2]["input"],
        tool_args(),
        "A3: tool_use.input is the parsed Gemini args object"
    );

    assert_eq!(
        body["stop_reason"], "tool_use",
        "A3: STOP-after-functionCall must project to `stop_reason: tool_use`"
    );

    let input_tokens = body["usage"]["input_tokens"]
        .as_u64()
        .unwrap_or_else(|| panic!("A3: usage.input_tokens must be present: {body}"));
    let cache_read = body["usage"]["cache_read_input_tokens"]
        .as_u64()
        .unwrap_or_else(|| panic!("A3: usage.cache_read_input_tokens must be an integer: {body}"));
    assert_eq!(cache_read, CACHED_TOKENS as u64, "A3: cached tokens are reported separately");
    assert_eq!(
        input_tokens + cache_read,
        PROMPT_TOKENS as u64,
        "A3: fresh input + cache read must account for Gemini's promptTokenCount"
    );
    assert_eq!(
        body["usage"]["output_tokens"], COMPLETION_TOKENS,
        "A3: Anthropic bills thinking as output, so output_tokens = candidates + thoughts"
    );
}

// ===========================================================================
// A4 — /v1/messages, streaming
// ===========================================================================

#[tokio::test]
async fn a4_messages_stream_emits_full_anthropic_event_sequence() {
    let h = spawn_harness(Mode::Full).await;
    let (status, raw) = h
        .post(
            "/v1/messages",
            json!({
                "model": MODEL,
                "max_tokens": 1024,
                "stream": true,
                "messages": [{"role": "user", "content": PROMPT}]
            }),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "A4: body={raw}");

    let frames = parse_sse(&raw, "A4");
    assert!(frames.len() > 5, "A4: a tool+thinking turn needs many events: {raw}");
    for frame in &frames {
        let name = frame
            .event
            .as_deref()
            .unwrap_or_else(|| panic!("A4: Anthropic SSE frames carry an `event:` name: {frame:?}"));
        assert_eq!(
            frame.data["type"].as_str(),
            Some(name),
            "A4: the `event:` name and the payload `type` must agree ({name})"
        );
    }

    let names: Vec<&str> = frames.iter().map(|f| f.event.as_deref().unwrap()).collect();
    assert_eq!(names.first(), Some(&"message_start"), "A4: sequence starts with message_start");
    assert_eq!(names.last(), Some(&"message_stop"), "A4: sequence ends with message_stop");
    assert_eq!(
        names.iter().filter(|n| **n == "message_stop").count(),
        1,
        "A4: exactly one message_stop: {names:?}"
    );
    assert_eq!(
        names.iter().filter(|n| **n == "message_start").count(),
        1,
        "A4: exactly one message_start: {names:?}"
    );

    let start = &frames[0].data;
    assert_eq!(start["message"]["role"], "assistant");
    assert_eq!(start["message"]["model"], MODEL);

    // Content blocks: thinking → text → tool_use, contiguous indices 0,1,2.
    let mut block_starts: Vec<(String, u64)> = Vec::new();
    let mut thinking = String::new();
    let mut text = String::new();
    let mut partial_json = String::new();
    let mut tool_block_seen = false;
    let mut stop_reason: Option<String> = None;
    let mut output_tokens: Option<u64> = None;
    let mut cache_read: Option<u64> = None;

    for frame in &frames {
        let name = frame.event.as_deref().unwrap();
        let data = &frame.data;
        match name {
            "content_block_start" => {
                let kind = data["content_block"]["type"].as_str().unwrap_or("<missing>");
                block_starts.push((
                    kind.to_string(),
                    data["index"].as_u64().expect("A4: content_block_start needs an index"),
                ));
                if kind == "tool_use" {
                    tool_block_seen = true;
                    assert_eq!(
                        data["content_block"]["id"], TOOL_ID,
                        "A4: the tool_use block must carry the upstream functionCall id: {data}"
                    );
                    assert_eq!(data["content_block"]["name"], TOOL_NAME);
                }
            }
            "content_block_delta" => {
                let delta = &data["delta"];
                match delta["type"].as_str() {
                    Some("thinking_delta") => thinking.push_str(
                        delta["thinking"].as_str().unwrap_or_default(),
                    ),
                    Some("text_delta") => {
                        text.push_str(delta["text"].as_str().unwrap_or_default())
                    }
                    Some("input_json_delta") => {
                        partial_json.push_str(delta["partial_json"].as_str().unwrap_or_default())
                    }
                    // Official Anthropic closes a thinking block with a `signature_delta`; the chat
                    // pivot carries no upstream signature, so it rides on the thinking block only.
                    Some("signature_delta") => assert_eq!(
                        data["index"], 0,
                        "A4: `signature_delta` may only belong to the thinking block: {data}"
                    ),
                    other => panic!("A4: unexpected delta type {other:?} in {data}"),
                }
            }
            "message_delta" => {
                stop_reason = data["delta"]["stop_reason"].as_str().map(str::to_string);
                output_tokens = data["usage"]["output_tokens"].as_u64();
                cache_read = data["usage"]["cache_read_input_tokens"].as_u64();
            }
            _ => {}
        }
    }

    assert_eq!(
        block_starts
            .iter()
            .map(|(kind, _)| kind.as_str())
            .collect::<Vec<_>>(),
        vec!["thinking", "text", "tool_use"],
        "A4: expected thinking → text → tool_use blocks, got {block_starts:?}"
    );
    assert_eq!(
        block_starts.iter().map(|(_, idx)| *idx).collect::<Vec<_>>(),
        vec![0, 1, 2],
        "A4: block indices must be contiguous and equal the final content order"
    );

    assert_eq!(thinking, THOUGHT, "A4: thinking_delta deltas must equal the upstream thought");
    assert_eq!(text, ANSWER, "A4: text_delta deltas must equal the upstream text");
    assert!(tool_block_seen, "A4: no tool_use content_block_start was emitted");
    assert_eq!(
        parsed_arguments(&partial_json, "A4"),
        tool_args(),
        "A4: input_json_delta partial_json must reassemble the Gemini args object"
    );

    assert_eq!(
        stop_reason.as_deref(),
        Some("tool_use"),
        "A4: message_delta must carry stop_reason=tool_use for a function-call turn"
    );
    assert_eq!(
        output_tokens,
        Some(COMPLETION_TOKENS as u64),
        "A4: message_delta usage must report completion tokens (candidates + thoughts)"
    );
    assert_eq!(
        cache_read,
        Some(CACHED_TOKENS as u64),
        "A4: message_delta usage must report the cached prompt tokens"
    );

    // Every opened block must be closed, in order.
    let stops: Vec<u64> = frames
        .iter()
        .filter(|f| f.event.as_deref() == Some("content_block_stop"))
        .map(|f| f.data["index"].as_u64().expect("A4: content_block_stop index"))
        .collect();
    assert_eq!(stops, vec![0, 1, 2], "A4: each content block must be stopped once, in order");
}

// ===========================================================================
// A5 — /v1/responses, non-streaming
// ===========================================================================

#[tokio::test]
async fn a5_responses_non_stream_has_reasoning_message_and_function_call_items() {
    let h = spawn_harness(Mode::Full).await;
    let (status, body) = h
        .post_json(
            "/v1/responses",
            json!({"model": MODEL, "input": PROMPT}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "A5: body={body}");
    assert_eq!(body["object"], "response");
    assert_eq!(body["status"], "completed", "A5: a served turn must be `completed`: {body}");
    assert_eq!(body["model"], MODEL);

    let output = body["output"]
        .as_array()
        .unwrap_or_else(|| panic!("A5: `output` must be an array: {body}"));
    let types: Vec<&str> = output
        .iter()
        .map(|item| item["type"].as_str().unwrap_or("<missing>"))
        .collect();
    assert_eq!(
        types,
        vec!["reasoning", "message", "function_call"],
        "A5: expected reasoning → message → function_call items, got {body}"
    );

    let reasoning = &output[0];
    let reasoning_text = reasoning["content"]
        .as_array()
        .and_then(|parts| parts.first())
        .map(|part| part["text"].as_str().unwrap_or_default())
        .unwrap_or_default();
    assert_eq!(reasoning_text, THOUGHT, "A5: the reasoning item must carry the thought text");

    let message = &output[1];
    assert_eq!(message["role"], "assistant");
    let text = message["content"]
        .as_array()
        .and_then(|parts| parts.first())
        .map(|part| {
            assert_eq!(part["type"], "output_text", "A5: message parts are `output_text`");
            part["text"].as_str().unwrap_or_default().to_string()
        })
        .unwrap_or_default();
    assert_eq!(text, ANSWER, "A5: the message item must carry the visible answer");

    let call = &output[2];
    assert_eq!(call["name"], TOOL_NAME);
    assert_eq!(
        call["call_id"], TOOL_ID,
        "A5: the function call must expose the upstream functionCall.id as `call_id`"
    );
    assert_eq!(
        parsed_arguments(
            call["arguments"]
                .as_str()
                .unwrap_or_else(|| panic!("A5: `arguments` must be a JSON string: {call}")),
            "A5"
        ),
        tool_args()
    );

    assert_eq!(body["usage"]["input_tokens"], PROMPT_TOKENS);
    assert_eq!(body["usage"]["output_tokens"], COMPLETION_TOKENS);
    assert_eq!(body["usage"]["total_tokens"], TOTAL_TOKENS);
    assert_eq!(
        body["usage"]["output_tokens_details"]["reasoning_tokens"], THOUGHT_TOKENS,
        "A5: thought tokens must be reported in `output_tokens_details.reasoning_tokens`"
    );
    assert_eq!(
        body["usage"]["input_tokens_details"]["cached_tokens"], CACHED_TOKENS,
        "A5: cached prompt tokens must be reported in `input_tokens_details.cached_tokens`"
    );
}

// ===========================================================================
// A6 — /v1/responses, streaming
// ===========================================================================

#[tokio::test]
async fn a6_responses_stream_emits_deltas_function_call_done_and_completed() {
    let h = spawn_harness(Mode::Full).await;
    let (status, raw) = h
        .post(
            "/v1/responses",
            json!({"model": MODEL, "input": PROMPT, "stream": true}),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "A6: body={raw}");

    let frames = parse_sse(&raw, "A6");
    assert!(frames.len() > 3, "A6: a tool+reasoning turn needs many events: {raw}");

    // Every Responses event carries a strictly increasing `sequence_number`.
    let mut last_seq: Option<u64> = None;
    for frame in &frames {
        let seq = frame.data["sequence_number"]
            .as_u64()
            .unwrap_or_else(|| panic!("A6: every event needs a sequence_number: {}", frame.data));
        if let Some(previous) = last_seq {
            assert!(
                seq > previous,
                "A6: sequence_number must be strictly increasing ({previous} then {seq})"
            );
        }
        last_seq = Some(seq);
        assert_eq!(
            frame.data["type"].as_str(),
            frame.event.as_deref(),
            "A6: event name and payload type must agree"
        );
    }

    let names: Vec<&str> = frames.iter().map(|f| f.event.as_deref().unwrap()).collect();
    assert_eq!(names.first(), Some(&"response.created"), "A6: stream opens with response.created");
    assert_eq!(
        names.last(),
        Some(&"response.completed"),
        "A6: stream must end with the response.completed terminal event: {names:?}"
    );

    let mut text = String::new();
    let mut reasoning = String::new();
    for frame in &frames {
        match frame.event.as_deref() {
            Some("response.output_text.delta") => {
                text.push_str(frame.data["delta"].as_str().unwrap_or_default())
            }
            Some("response.reasoning_summary_text.delta") => {
                reasoning.push_str(frame.data["delta"].as_str().unwrap_or_default())
            }
            _ => {}
        }
    }
    assert_eq!(text, ANSWER, "A6: `response.output_text.delta` frames must rebuild the answer");
    assert_eq!(
        reasoning, THOUGHT,
        "A6: reasoning must stream as `response.reasoning_summary_text.delta`"
    );

    let done_items: Vec<&Value> = frames
        .iter()
        .filter(|f| f.event.as_deref() == Some("response.output_item.done"))
        .map(|f| &f.data["item"])
        .collect();
    assert!(
        done_items
            .iter()
            .any(|item| item["type"] == "reasoning"),
        "A6: the reasoning item must be finalized with `response.output_item.done`"
    );

    // The function_call item is opened explicitly and its arguments stream as
    // `response.function_call_arguments.delta`. NOTE: this gateway (all protocols, shared
    // chat→Responses converter) does not emit a separate `response.output_item.done` for
    // function_call items — the item is finalized inside `response.completed.response.output`.
    // Asserted here is what the Gemini adapter must guarantee: the upstream functionCall id/name
    // and the full arguments object reach the client, and the item is `completed` in the terminal
    // response. See .agents/receipts/2026-11-15-gemini-e2e.md §observations.
    let added_call = frames
        .iter()
        .find(|f| {
            f.event.as_deref() == Some("response.output_item.added")
                && f.data["item"]["type"] == "function_call"
        })
        .unwrap_or_else(|| panic!("A6: no `response.output_item.added` for a function_call item: {raw}"));
    assert_eq!(
        added_call.data["item"]["call_id"], TOOL_ID,
        "A6: the opened function_call item must carry the upstream functionCall id: {}",
        added_call.data
    );
    assert_eq!(added_call.data["item"]["name"], TOOL_NAME);

    let mut streamed_arguments = String::new();
    for frame in &frames {
        if frame.event.as_deref() == Some("response.function_call_arguments.delta") {
            streamed_arguments.push_str(frame.data["delta"].as_str().unwrap_or_default());
        }
    }
    assert_eq!(
        parsed_arguments(&streamed_arguments, "A6"),
        tool_args(),
        "A6: `response.function_call_arguments.delta` frames must rebuild the Gemini args object"
    );

    let completed = &frames.last().unwrap().data;
    assert_eq!(
        completed["response"]["status"], "completed",
        "A6: the terminal response object must be `completed`"
    );
    let terminal_call = completed["response"]["output"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["type"] == "function_call"))
        .unwrap_or_else(|| {
            panic!(
                "A6: the terminal response must contain the completed function_call item: {}",
                completed["response"]
            )
        });
    assert_eq!(terminal_call["status"], "completed");
    assert_eq!(terminal_call["call_id"], TOOL_ID);
    assert_eq!(terminal_call["name"], TOOL_NAME);
    assert_eq!(
        parsed_arguments(
            terminal_call["arguments"].as_str().unwrap_or_default(),
            "A6"
        ),
        tool_args()
    );
    assert_eq!(
        completed["response"]["usage"]["output_tokens"], COMPLETION_TOKENS,
        "A6: the terminal response must carry real usage"
    );
}

// ===========================================================================
// A7 — outbound native Gemini contract (URL / auth header / body)
// ===========================================================================

#[tokio::test]
async fn a7_outbound_requests_are_native_gemini_shaped() {
    let h = spawn_harness(Mode::Full).await;

    let (status, body) = h
        .post_json(
            "/v1/chat/completions",
            json!({
                "model": MODEL,
                "messages": [
                    {"role": "system", "content": SYSTEM},
                    {"role": "user", "content": PROMPT}
                ],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": TOOL_NAME,
                        "description": "Get the current weather for a city",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "location": {"type": "string"},
                                "unit": {"type": "string", "enum": ["celsius", "fahrenheit"]}
                            },
                            "required": ["location"]
                        }
                    }
                }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "A7: non-streaming call failed: {body}");

    let (status, raw) = h
        .post(
            "/v1/chat/completions",
            json!({"model": MODEL, "stream": true, "messages": [{"role": "user", "content": PROMPT}]}),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "A7: streaming call failed: {raw}");

    let captured = h.upstream.captured();
    assert_eq!(
        captured.len(),
        2,
        "A7: the gateway must have made exactly two upstream calls, saw {}",
        captured.len()
    );

    let non_stream = &captured[0];
    assert_native_outbound(non_stream, false, "A7/non-stream");

    // system → systemInstruction.parts[].text (official Gemini request shape).
    assert_eq!(
        non_stream.body["systemInstruction"]["parts"][0]["text"], SYSTEM,
        "A7: the system message must become `systemInstruction.parts[0].text`: {}",
        non_stream.body
    );
    // user turn text lands in contents[0].parts[0].text.
    assert_eq!(
        non_stream.body["contents"][0]["parts"][0]["text"], PROMPT,
        "A7: the user text must be the first content part: {}",
        non_stream.body
    );
    // OpenAI tools → Gemini `tools[0].functionDeclarations`.
    assert_eq!(
        non_stream.body["tools"][0]["functionDeclarations"][0]["name"], TOOL_NAME,
        "A7: OpenAI function tools must become `functionDeclarations`: {}",
        non_stream.body
    );
    assert!(
        non_stream.body["contents"]
            .as_array()
            .is_some_and(|turns| turns
                .iter()
                .all(|turn| turn.get("role").and_then(Value::as_str) != Some("system"))),
        "A7: system prompts go to `systemInstruction`, never into `contents`: {}",
        non_stream.body
    );

    let streaming = &captured[1];
    assert_native_outbound(streaming, true, "A7/stream");
}

// ===========================================================================
// A8 — upstream 400 API_KEY_INVALID must surface as a 400, not a gateway fault
// ===========================================================================

#[tokio::test]
async fn a8_upstream_invalid_api_key_surfaces_as_client_400() {
    let h = spawn_harness(Mode::BadApiKey).await;
    let (status, raw) = h
        .post(
            "/v1/chat/completions",
            json!({"model": MODEL, "messages": [{"role": "user", "content": PROMPT}]}),
            false,
        )
        .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "A8: an upstream 400 API_KEY_INVALID is the caller's problem, not a 500/502/503: {raw}"
    );
    let body: Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("A8: the error body must be JSON ({e}): {raw}"));
    assert!(
        body.get("error").is_some(),
        "A8: the error envelope must carry an `error` object: {body}"
    );
    let lowered = raw.to_ascii_lowercase();
    assert!(
        lowered.contains("api key not valid") || lowered.contains("api_key_invalid"),
        "A8: the upstream error semantics must be preserved for the caller: {raw}"
    );
    for forbidden in ["status 500", "\"code\": 500", "internal server error"] {
        assert!(
            !lowered.contains(forbidden),
            "A8: an invalid key must not be reported as a gateway 500 (`{forbidden}`): {raw}"
        );
    }
}

// ===========================================================================
// A9 — upstream 429 RESOURCE_EXHAUSTED (no rpm/tpm wording) stays a 429
// ===========================================================================

#[tokio::test]
async fn a9_upstream_resource_exhausted_stays_a_429() {
    let h = spawn_harness(Mode::QuotaExhausted).await;
    for (path, payload, anthropic) in [
        (
            "/v1/chat/completions",
            json!({"model": MODEL, "messages": [{"role": "user", "content": PROMPT}]}),
            false,
        ),
        (
            "/v1/messages",
            json!({"model": MODEL, "max_tokens": 64, "messages": [{"role": "user", "content": PROMPT}]}),
            true,
        ),
        (
            "/v1/responses",
            json!({"model": MODEL, "input": PROMPT}),
            false,
        ),
    ] {
        let (status, raw) = h.post(path, payload, anthropic).await;
        assert_eq!(
            status,
            StatusCode::TOO_MANY_REQUESTS,
            "A9[{path}]: Gemini's quota 429 must reach the caller as 429, not 5xx: {raw}"
        );
        let lowered = raw.to_ascii_lowercase();
        for forbidden in ["status 500", "\"code\": 500", "internal server error"] {
            assert!(
                !lowered.contains(forbidden),
                "A9[{path}]: a quota rejection must never be rendered as a 500 (`{forbidden}`): {raw}"
            );
        }
        let body: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("A9[{path}]: error body must be JSON ({e}): {raw}"));
        assert!(
            body.get("error").is_some(),
            "A9[{path}]: the 429 body must carry an `error` envelope: {body}"
        );
    }
}

// ===========================================================================
// A10 — no cross-request bleed between two Responses streams
// ===========================================================================

#[tokio::test]
async fn a10_responses_streams_do_not_bleed_ids_or_sequences() {
    let h = spawn_harness(Mode::Full).await;

    let mut per_stream: Vec<(String, Vec<u64>)> = Vec::new();
    for _ in 0..2 {
        let (status, raw) = h
            .post(
                "/v1/responses",
                json!({"model": MODEL, "input": PROMPT, "stream": true}),
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "A10: body={raw}");
        let frames = parse_sse(&raw, "A10");
        let created = frames
            .iter()
            .find(|f| f.event.as_deref() == Some("response.created"))
            .unwrap_or_else(|| panic!("A10: no response.created frame: {raw}"));
        let id = created.data["response"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("A10: response.created must carry an id: {}", created.data))
            .to_string();
        // Every `response`-bearing frame of this stream must reference this same id.
        for frame in &frames {
            if let Some(other) = frame.data["response"]["id"].as_str() {
                assert_eq!(
                    other, id,
                    "A10: a single stream must carry one response id, saw `{other}` after `{id}`"
                );
            }
        }
        let seqs: Vec<u64> = frames
            .iter()
            .map(|f| {
                f.data["sequence_number"]
                    .as_u64()
                    .unwrap_or_else(|| panic!("A10: missing sequence_number: {}", f.data))
            })
            .collect();
        per_stream.push((id, seqs));
    }

    assert_ne!(
        per_stream[0].0, per_stream[1].0,
        "A10: two requests must not share one response id ({})",
        per_stream[0].0
    );
    for (id, seqs) in &per_stream {
        let mut sorted = seqs.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seqs.len(),
            "A10[{id}]: duplicate sequence_number inside one stream"
        );
        assert!(
            seqs.windows(2).all(|w| w[0] < w[1]),
            "A10[{id}]: sequence_number must increase monotonically: {seqs:?}"
        );
    }
}
