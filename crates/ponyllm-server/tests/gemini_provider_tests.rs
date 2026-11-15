//! L2 验收：`UpstreamProtocol::Gemini` 三下游协议接线（非流式 + 流式）。
//!
//! 契约：`.agents/notes/proposed/feature/2026-11-15-gemini-native-upstream-protocol.md`
//! —— Gemini Developer API 作为 provider 的原生上游协议，三条下游路由
//! （`/v1/chat/completions`、`/v1/messages`、`/v1/responses`）都必须可用：
//!
//! * 出口 URL 是原生 Gemini 形状 `{base}/models/{model}:generateContent`
//!   （流式为 `:streamGenerateContent?alt=sse`），模型名走路径、**不进 body**；
//! * 出口鉴权只用 `x-goog-api-key`，绝不夹带 `authorization` / `x-api-key` /
//!   `anthropic-version`（发给 Google 网关是噪声，最坏是与 key 冲突的凭据）；
//! * 三协议统一走 Chat 枢轴：Gemini ↔ Chat 的翻译由 protocol 层负责，路由层
//!   只做 Chat → 下游协议的投影；
//! * 流式必须自带终止语义：chat 出口 `data: [DONE]`、messages 出口
//!   `message_stop`、responses 出口 `response.completed`。
//!
//! 本文件只经由公开 HTTP 面（`create_app` + 本地 axum mock 上游）做黑盒断言：
//! 每条断言都对齐线上真实可观测的 URL / header / body 与下游响应字段。

use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use ponyllm_config::AuthMode;
use ponyllm_core::pool::*;
use ponyllm_server::{create_app, AppState, GatewayConfig, ModelSpec, ProviderConfig};
use serde_json::{json, Value};

const PROVIDER: &str = "gemini-direct";
const MODEL: &str = "test-gemini";
const API_KEY: &str = "sk-gemini-secret";
const ANSWER: &str = "pong";

/// 上游实际收到的请求快照。
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

#[derive(Clone, Default)]
struct UpstreamState {
    captured: Arc<Mutex<Vec<Captured>>>,
}

impl UpstreamState {
    fn take_last(&self) -> Captured {
        self.captured
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("mock Gemini upstream must have received at least one request")
    }
}

/// 原生 Gemini `generateContent` 响应（无 Anthropic/Antigravity 信封）。
fn gemini_json_response() -> Value {
    json!({
        "candidates": [{
            "content": { "parts": [{"text": ANSWER}], "role": "model" },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {
            "promptTokenCount": 7,
            "candidatesTokenCount": 2,
            "totalTokenCount": 9
        },
        "modelVersion": MODEL,
        "responseId": "gemini-resp-1"
    })
}

/// 原生 Gemini SSE 帧序列：两段文本 + 末帧 `finishReason` + 终止 `[DONE]`。
fn gemini_sse_body() -> String {
    let frames = [
        json!({
            "candidates": [{"content": {"parts": [{"text": "po"}], "role": "model"}, "index": 0}],
            "modelVersion": MODEL,
            "responseId": "gemini-resp-1"
        }),
        json!({
            "candidates": [{
                "content": {"parts": [{"text": "ng"}], "role": "model"},
                "finishReason": "STOP",
                "index": 0
            }],
            "usageMetadata": {
                "promptTokenCount": 7,
                "candidatesTokenCount": 2,
                "totalTokenCount": 9
            },
            "modelVersion": MODEL,
            "responseId": "gemini-resp-1"
        }),
    ];
    let mut out = String::new();
    for f in frames {
        out.push_str(&format!("data: {}\n\n", f));
    }
    out.push_str("data: [DONE]\n\n");
    out
}

/// 原生 Gemini 工具调用响应（非流式）：functionCall + finishReason=STOP。
fn gemini_tool_json_response() -> Value {
    json!({
        "candidates": [{
            "content": {
                "parts": [{"functionCall": {"name": "get_weather", "args": {"location": "Paris"}}}],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {
            "promptTokenCount": 11,
            "candidatesTokenCount": 5,
            "totalTokenCount": 16
        },
        "modelVersion": MODEL,
        "responseId": "gemini-tool-1"
    })
}

/// 原生 Gemini 工具调用 SSE：**关键回归形状** —— `functionCall` 在第一帧，
/// 独立的 `finishReason: STOP` 在第二帧。逐帧无状态映射会把第二帧错判成
/// `finish_reason: stop`，让下游把工具轮次当普通文本结束。
fn gemini_tool_sse_body() -> String {
    let frames = [
        json!({
            "candidates": [{
                "content": {
                    "parts": [{"functionCall": {"name": "get_weather", "args": {"location": "Paris"}}}],
                    "role": "model"
                },
                "index": 0
            }],
            "modelVersion": MODEL,
            "responseId": "gemini-tool-1"
        }),
        json!({
            "candidates": [{
                "content": {"parts": [], "role": "model"},
                "finishReason": "STOP",
                "index": 0
            }],
            "usageMetadata": {
                "promptTokenCount": 11,
                "candidatesTokenCount": 5,
                "totalTokenCount": 16
            },
            "modelVersion": MODEL,
            "responseId": "gemini-tool-1"
        }),
    ];
    let mut out = String::new();
    for f in frames {
        out.push_str(&format!("data: {}\n\n", f));
    }
    out.push_str("data: [DONE]\n\n");
    out
}

/// Mock Gemini 上游：按 URL 动词分流（`:streamGenerateContent` → SSE），
/// 并把每次请求的 method/path/query/headers/body 原样记录下来。
///
/// 请求带非空 `tools` 时返回工具调用形状（functionCall 帧 + 独立 STOP 帧），
/// 否则返回纯文本形状。
async fn mock_gemini(
    State(state): State<UpstreamState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body_val: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let wants_tools = body_val
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|t| !t.is_empty());
    state.captured.lock().unwrap().push(Captured {
        method: method.to_string(),
        path: uri.path().to_string(),
        query: uri.query().map(str::to_string),
        headers: headers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect(),
        body: body_val,
    });

    let streaming = uri.path().contains("streamGenerateContent");
    if streaming {
        let sse = if wants_tools {
            gemini_tool_sse_body()
        } else {
            gemini_sse_body()
        };
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(Body::from(sse))
            .unwrap()
    } else if wants_tools {
        axum::Json(gemini_tool_json_response()).into_response()
    } else {
        axum::Json(gemini_json_response()).into_response()
    }
}

struct Harness {
    base: String,
    upstream: UpstreamState,
}

impl Harness {
    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("build reqwest client")
    }

    async fn post(&self, path: &str, body: Value) -> (reqwest::StatusCode, String) {
        let resp = Self::client()
            .post(format!("{}{}", self.base, path))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .unwrap_or_else(|e| panic!("POST {path} failed: {e}"));
        let status = resp.status();
        let text = resp
            .text()
            .await
            .unwrap_or_else(|e| panic!("reading body of POST {path} failed: {e}"));
        (status, text)
    }
}

/// 起本地 mock Gemini 上游 + 单 provider 网关（`default_protocol = Gemini`）。
async fn spawn_harness() -> Harness {
    let upstream_state = UpstreamState::default();
    let mock = Router::new()
        .fallback(any(mock_gemini))
        .with_state(upstream_state.clone());
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, mock).await.unwrap();
    });

    let pool = Arc::new(KeyPool::new(PROVIDER, RoutingStrategy::RoundRobin));
    pool.add_key(ApiKeyEntry::new("gem-key", API_KEY, 1, 10));

    let mut config = GatewayConfig::default();
    config.auth_mode = AuthMode::Open;
    config.providers.insert(
        PROVIDER.to_string(),
        ProviderConfig {
            base_url: format!("http://{}/v1beta", upstream_addr),
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
    let gw_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(gw_listener, app).await.unwrap();
    });

    Harness {
        base: format!("http://{}", gw_addr),
        upstream: upstream_state,
    }
}

/// 出口请求的 Gemini 契约（URL / 鉴权头 / body 形状）。三协议、六条用例共用。
#[track_caller]
fn assert_gemini_outbound(cap: &Captured, streaming: bool, case: &str) {
    assert_eq!(cap.method, "POST", "{case}: 出口必须 POST");
    let expected_verb = if streaming {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    assert_eq!(
        cap.path,
        format!("/v1beta/models/{MODEL}:{expected_verb}"),
        "{case}: 出口 URL 必须是原生 Gemini 形状 {{base}}/models/{{model}}:{expected_verb}；实际 {}",
        cap.path
    );
    if streaming {
        assert_eq!(
            cap.query.as_deref(),
            Some("alt=sse"),
            "{case}: 流式出口必须带 alt=sse（否则上游返回 JSON 数组而非 SSE）"
        );
    } else {
        assert!(
            cap.query.is_none(),
            "{case}: 非流式出口不应带 query，实际 {:?}",
            cap.query
        );
    }

    assert_eq!(
        cap.header("x-goog-api-key"),
        Some(API_KEY),
        "{case}: 鉴权必须走 x-goog-api-key"
    );
    for forbidden in ["authorization", "x-api-key", "anthropic-version"] {
        assert!(
            cap.header(forbidden).is_none(),
            "{case}: Gemini 出口不得携带 `{forbidden}` 头，实际 {:?}",
            cap.header(forbidden)
        );
    }

    assert!(
        cap.body.get("contents").is_some_and(|c| c.is_array()),
        "{case}: 出口 body 必须含 contents 数组，实际 {}",
        cap.body
    );
    assert!(
        cap.body.get("model").is_none(),
        "{case}: 模型名走 URL 路径，body 不得含 model 字段，实际 {}",
        cap.body
    );
    let first_text = cap
        .body
        .pointer("/contents/0/parts/0/text")
        .and_then(Value::as_str);
    assert_eq!(
        first_text,
        Some("ping"),
        "{case}: 下游 user 文本必须落到 contents[0].parts[0].text"
    );
}

/// 解析 SSE body 为 `(event_name, data_json)`；任何 data 行不是合法 JSON 即失败。
#[track_caller]
fn parse_sse(body: &str, case: &str) -> Vec<(Option<String>, Value)> {
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
        let Some(data) = data else {
            panic!("{case}: SSE 帧缺 data 行，原始块：{block}");
        };
        if data == "[DONE]" {
            out.push((event, json!("[DONE]")));
            continue;
        }
        let parsed: Value = serde_json::from_str(&data)
            .unwrap_or_else(|e| panic!("{case}: 客户端线上出现非法 JSON（{e}）：{data}"));
        out.push((event, parsed));
    }
    out
}

// ===========================================================================
// A. `/v1/chat/completions`
// ===========================================================================

#[tokio::test]
async fn a1_chat_non_stream_maps_gemini_to_openai() {
    let h = spawn_harness().await;
    let (status, text) = h
        .post(
            "/v1/chat/completions",
            json!({
                "model": MODEL,
                "messages": [{"role": "user", "content": "ping"}]
            }),
        )
        .await;
    assert_eq!(status, 200, "A1: 非流式 chat 必须 200，body={text}");
    let body: Value = serde_json::from_str(&text).expect("A1: chat 响应必须是 JSON");

    assert_eq!(
        body.pointer("/choices/0/message/content")
            .and_then(Value::as_str),
        Some(ANSWER),
        "A1: candidates[].content.parts[].text 必须投影到 choices[0].message.content，实际 {body}"
    );
    assert_eq!(
        body.pointer("/choices/0/finish_reason")
            .and_then(Value::as_str),
        Some("stop"),
        "A1: finishReason=STOP 必须映射为 finish_reason=stop，实际 {body}"
    );
    assert_eq!(
        body.pointer("/usage/prompt_tokens").and_then(Value::as_u64),
        Some(7),
        "A1: usageMetadata.promptTokenCount 必须落到 usage.prompt_tokens，实际 {body}"
    );
    assert_eq!(
        body.pointer("/usage/completion_tokens")
            .and_then(Value::as_u64),
        Some(2),
        "A1: usageMetadata.candidatesTokenCount 必须落到 usage.completion_tokens，实际 {body}"
    );

    assert_gemini_outbound(&h.upstream.take_last(), false, "A1");
}

#[tokio::test]
async fn a2_chat_stream_translates_gemini_sse_to_openai_chunks() {
    let h = spawn_harness().await;
    let (status, text) = h
        .post(
            "/v1/chat/completions",
            json!({
                "model": MODEL,
                "stream": true,
                "messages": [{"role": "user", "content": "ping"}]
            }),
        )
        .await;
    assert_eq!(status, 200, "A2: 流式 chat 必须 200，body={text}");

    let frames = parse_sse(&text, "A2");
    assert!(
        frames.iter().any(|(_, d)| d == &json!("[DONE]")),
        "A2: OpenAI 兼容流必须以 data: [DONE] 终止，实际 {text}"
    );
    let deltas: String = frames
        .iter()
        .filter(|(_, d)| d != &json!("[DONE]"))
        .filter_map(|(_, d)| {
            d.pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        deltas, "pong",
        "A2: 分片文本必须按序拼回 {ANSWER}，实际 {text}"
    );
    assert!(
        frames.iter().any(|(_, d)| d
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
            == Some("stop")),
        "A2: 流中必须出现 finish_reason=stop（不允许只有 [DONE]），实际 {text}"
    );

    assert_gemini_outbound(&h.upstream.take_last(), true, "A2");
}

/// OpenAI 形态的工具声明（`tools[].function`）。
fn openai_tools() -> Value {
    json!([{
        "type": "function",
        "function": {
            "name": "get_weather",
            "description": "Get the current weather for a location",
            "parameters": {
                "type": "object",
                "properties": {"location": {"type": "string"}},
                "required": ["location"]
            }
        }
    }])
}

/// Anthropic 形态的工具声明（`tools[].input_schema`）。
fn anthropic_tools() -> Value {
    json!([{
        "name": "get_weather",
        "description": "Get the current weather for a location",
        "input_schema": {
            "type": "object",
            "properties": {"location": {"type": "string"}},
            "required": ["location"]
        }
    }])
}

/// 回归（Lead 真实 E2E 缺陷）：上游 Gemini 把 `functionCall` 与
/// `finishReason: STOP` 拆成两帧。终止帧必须被提升为 `finish_reason:
/// tool_calls`，否则下游会把工具轮次当普通文本结束。
#[tokio::test]
async fn a3_chat_stream_tool_call_finish_promotes_to_tool_calls() {
    let h = spawn_harness().await;
    let (status, text) = h
        .post(
            "/v1/chat/completions",
            json!({
                "model": MODEL,
                "stream": true,
                "messages": [{"role": "user", "content": "ping"}],
                "tools": openai_tools()
            }),
        )
        .await;
    assert_eq!(status, 200, "A3: 工具调用流式 chat 必须 200，body={text}");

    let frames = parse_sse(&text, "A3");
    let tool_names: Vec<&str> = frames
        .iter()
        .filter_map(|(_, d)| {
            d.pointer("/choices/0/delta/tool_calls/0/function/name")
                .and_then(Value::as_str)
        })
        .collect();
    assert!(
        tool_names.contains(&"get_weather"),
        "A3: 上游 functionCall 必须投影成 delta.tool_calls[].function.name，实际 {text}"
    );

    let finish_reasons: Vec<&str> = frames
        .iter()
        .filter_map(|(_, d)| {
            d.pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
        })
        .collect();
    assert!(
        finish_reasons.contains(&"tool_calls"),
        "A3: functionCall 与 STOP 分帧时终止帧必须是 finish_reason=tool_calls，实际 {finish_reasons:?}，body={text}"
    );
    assert!(
        !finish_reasons.contains(&"stop"),
        "A3: 工具调用轮次绝不能以 finish_reason=stop 结束，实际 {finish_reasons:?}"
    );

    assert_gemini_outbound(&h.upstream.take_last(), true, "A3");
}

/// 非流式同语义：单个 STOP 帧里既有 functionCall 又有 finishReason 时，
/// `finish_reason` 也必须是 `tool_calls`。
#[tokio::test]
async fn a4_chat_non_stream_tool_call_finish_is_tool_calls() {
    let h = spawn_harness().await;
    let (status, text) = h
        .post(
            "/v1/chat/completions",
            json!({
                "model": MODEL,
                "messages": [{"role": "user", "content": "ping"}],
                "tools": openai_tools()
            }),
        )
        .await;
    assert_eq!(status, 200, "A4: 工具调用非流式 chat 必须 200，body={text}");
    let body: Value = serde_json::from_str(&text).expect("A4: chat 响应必须是 JSON");
    assert_eq!(
        body.pointer("/choices/0/finish_reason")
            .and_then(Value::as_str),
        Some("tool_calls"),
        "A4: 带 functionCall 的响应 finish_reason 必须是 tool_calls，实际 {body}"
    );
    assert_eq!(
        body.pointer("/choices/0/message/tool_calls/0/function/name")
            .and_then(Value::as_str),
        Some("get_weather"),
        "A4: functionCall 必须投影到 message.tool_calls，实际 {body}"
    );
}

// ===========================================================================
// B. `/v1/messages`
// ===========================================================================

#[tokio::test]
async fn b1_messages_non_stream_projects_gemini_via_chat() {
    let h = spawn_harness().await;
    let (status, text) = h
        .post(
            "/v1/messages",
            json!({
                "model": MODEL,
                "max_tokens": 64,
                "messages": [{"role": "user", "content": "ping"}]
            }),
        )
        .await;
    assert_eq!(status, 200, "B1: 非流式 messages 必须 200，body={text}");
    let body: Value = serde_json::from_str(&text).expect("B1: messages 响应必须是 JSON");

    assert_eq!(body.get("type").and_then(Value::as_str), Some("message"));
    assert_eq!(
        body.pointer("/content/0/text").and_then(Value::as_str),
        Some(ANSWER),
        "B1: ANSWER 必须投影到 content[0].text，实际 {body}"
    );
    assert_eq!(
        body.get("stop_reason").and_then(Value::as_str),
        Some("end_turn"),
        "B1: STOP 必须映射为 Anthropic stop_reason=end_turn，实际 {body}"
    );
    assert_eq!(
        body.pointer("/usage/input_tokens").and_then(Value::as_u64),
        Some(7),
        "B1: promptTokenCount 必须落到 usage.input_tokens，实际 {body}"
    );
    assert_eq!(
        body.pointer("/usage/output_tokens").and_then(Value::as_u64),
        Some(2),
        "B1: candidatesTokenCount 必须落到 usage.output_tokens，实际 {body}"
    );

    assert_gemini_outbound(&h.upstream.take_last(), false, "B1");
}

#[tokio::test]
async fn b2_messages_stream_terminates_with_message_stop() {
    let h = spawn_harness().await;
    let (status, text) = h
        .post(
            "/v1/messages",
            json!({
                "model": MODEL,
                "max_tokens": 64,
                "stream": true,
                "messages": [{"role": "user", "content": "ping"}]
            }),
        )
        .await;
    assert_eq!(status, 200, "B2: 流式 messages 必须 200，body={text}");

    let frames = parse_sse(&text, "B2");
    let events: Vec<&str> = frames.iter().filter_map(|(e, _)| e.as_deref()).collect();
    assert!(
        events.contains(&"message_start"),
        "B2: Anthropic 流必须以 message_start 开场，实际事件 {events:?}"
    );
    assert!(
        events.contains(&"message_stop"),
        "B2: Anthropic 流必须以 message_stop 终止，实际事件 {events:?}"
    );
    let streamed: String = frames
        .iter()
        .filter(|(e, _)| e.as_deref() == Some("content_block_delta"))
        .filter_map(|(_, d)| d.pointer("/delta/text").and_then(Value::as_str))
        .collect();
    assert_eq!(
        streamed, ANSWER,
        "B2: content_block_delta 文本必须拼回 {ANSWER}，实际 {text}"
    );

    assert_gemini_outbound(&h.upstream.take_last(), true, "B2");
}

/// Lead E2E 缺陷的 Anthropic 侧回归：functionCall 与 STOP 分帧时，
/// `/v1/messages` 流的终止 `message_delta.stop_reason` 必须是 `tool_use`
/// （而不是 `end_turn`），且必须出现 `tool_use` 内容块。
#[tokio::test]
async fn b3_messages_stream_tool_call_stop_reason_is_tool_use() {
    let h = spawn_harness().await;
    let (status, text) = h
        .post(
            "/v1/messages",
            json!({
                "model": MODEL,
                "max_tokens": 64,
                "stream": true,
                "messages": [{"role": "user", "content": "ping"}],
                "tools": anthropic_tools()
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "B3: 工具调用流式 messages 必须 200，body={text}"
    );

    let frames = parse_sse(&text, "B3");
    let events: Vec<&str> = frames.iter().filter_map(|(e, _)| e.as_deref()).collect();
    assert!(
        events.contains(&"message_stop"),
        "B3: Anthropic 流必须以 message_stop 终止，实际事件 {events:?}\nbody={text}"
    );

    let block_types: Vec<&str> = frames
        .iter()
        .filter(|(e, _)| e.as_deref() == Some("content_block_start"))
        .filter_map(|(_, d)| d.pointer("/content_block/type").and_then(Value::as_str))
        .collect();
    assert!(
        block_types.contains(&"tool_use"),
        "B3: 工具调用必须投影成 tool_use 内容块，实际 {block_types:?}\nbody={text}"
    );

    let stop_reasons: Vec<&str> = frames
        .iter()
        .filter(|(e, _)| e.as_deref() == Some("message_delta"))
        .filter_map(|(_, d)| d.pointer("/delta/stop_reason").and_then(Value::as_str))
        .collect();
    assert_eq!(
        stop_reasons,
        vec!["tool_use"],
        "B3: 工具轮次的 message_delta.stop_reason 必须是 tool_use（functionCall 与 STOP 分帧），实际 {stop_reasons:?}\nbody={text}"
    );

    assert_gemini_outbound(&h.upstream.take_last(), true, "B3");
}

// ===========================================================================
// C. `/v1/responses`
// ===========================================================================

#[tokio::test]
async fn c1_responses_non_stream_projects_gemini_via_chat() {
    let h = spawn_harness().await;
    let (status, text) = h
        .post("/v1/responses", json!({"model": MODEL, "input": "ping"}))
        .await;
    assert_eq!(status, 200, "C1: 非流式 responses 必须 200，body={text}");
    let body: Value = serde_json::from_str(&text).expect("C1: responses 响应必须是 JSON");

    assert_eq!(
        body.get("status").and_then(Value::as_str),
        Some("completed"),
        "C1: Responses 成功响应 status 必须是 completed，实际 {body}"
    );
    let output_text = body
        .pointer("/output/0/content/0/text")
        .and_then(Value::as_str);
    assert_eq!(
        output_text,
        Some(ANSWER),
        "C1: ANSWER 必须投影到 output[0].content[0].text，实际 {body}"
    );
    assert_eq!(
        body.pointer("/usage/input_tokens").and_then(Value::as_u64),
        Some(7),
        "C1: promptTokenCount 必须落到 usage.input_tokens，实际 {body}"
    );

    assert_gemini_outbound(&h.upstream.take_last(), false, "C1");
}

#[tokio::test]
async fn c2_responses_stream_terminates_with_response_completed() {
    let h = spawn_harness().await;
    let (status, text) = h
        .post(
            "/v1/responses",
            json!({"model": MODEL, "input": "ping", "stream": true}),
        )
        .await;
    assert_eq!(status, 200, "C2: 流式 responses 必须 200，body={text}");

    let frames = parse_sse(&text, "C2");
    let events: Vec<&str> = frames.iter().filter_map(|(e, _)| e.as_deref()).collect();
    assert!(
        events.contains(&"response.completed"),
        "C2: Responses 流必须以 response.completed 终止，实际事件 {events:?}\nbody={text}"
    );
    let streamed: String = frames
        .iter()
        .filter(|(e, _)| e.as_deref() == Some("response.output_text.delta"))
        .filter_map(|(_, d)| d.get("delta").and_then(Value::as_str))
        .collect();
    assert_eq!(
        streamed, ANSWER,
        "C2: output_text.delta 片段必须拼回 {ANSWER}，实际 {text}"
    );

    assert_gemini_outbound(&h.upstream.take_last(), true, "C2");
}
