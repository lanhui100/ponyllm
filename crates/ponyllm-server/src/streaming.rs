//! SSE streaming helpers.
//!
//! The gateway must translate between OpenAI Chat Completions SSE and Anthropic
//! Messages SSE, and must *not* re-wrap already-prefixed upstream SSE frames.
//!
//! Upstream responses arrive as raw byte streams that are already SSE-framed
//! (`data: {...}\n\n` for OpenAI, `event: ...\ndata: {...}\n\n` for Anthropic).
//! This module provides:
//! - a tiny incremental SSE parser (`sse_event_stream`) so frames that are split
//!   across network chunks are reassembled and consumed one event at a time;
//! - OpenAI->Anthropic and Anthropic->OpenAI event translators that reuse the
//!   streaming FSMs from `ponyllm-protocol::translator::stream`.
//!
//! Bug fix background: the handlers previously wrapped every upstream byte chunk
//! in `axum::response::sse::Event::default().data(bytes)`, which produced
//! `data: data: {...}` double prefixes for OpenAI streams and silently returned
//! OpenAI `chat.completion.chunk` frames to Anthropic clients (broken event types).

use bytes::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use std::sync::Arc;
use std::time::Instant;
use parking_lot::Mutex;
use ponyllm_core::telemetry::{
    gap_percentiles, EventBus, EventCtx, GatewayEvent, StageTimings, StreamFlowSample,
};
use ponyllm_protocol::anthropic::messages::MessageStreamEvent;
use ponyllm_protocol::openai::chat::ChatCompletionChunk;
use ponyllm_protocol::openai::responses::ResponseStreamEvent;
use ponyllm_protocol::translator::{
    antigravity_chunk_to_chat_chunk,
    AnthropicStreamToChatFsm, AnthropicToResponsesFsm, ChatStreamToAnthropicFsm,
    ChatToResponsesFsm, ResponsesToAnthropicFsm, ResponsesToChatFsm,
};

/// A single parsed SSE frame.
#[derive(Debug, Clone)]
pub struct SseEvent {
    /// The `event:` field; defaults to `"message"` when absent (OpenAI style).
    pub event: String,
    /// The concatenated `data:` payload lines.
    pub data: String,
}

/// Pass through an upstream SSE byte stream unchanged (same protocol on both
/// ends — e.g. OpenAI upstream -> OpenAI client, or Anthropic upstream ->
/// Anthropic client). The upstream frames are already correctly prefixed, so
/// we must NOT re-wrap them.
pub fn passthrough_sse<S, E>(
    stream: S,
) -> impl Stream<Item = Result<Bytes, E>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    stream
}

/// Find the byte length of the first complete SSE event (bounded by a blank line) in `buf`.
fn find_sse_boundary(buf: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 1 < buf.len() {
        // \n\n boundary
        if buf[i] == b'\n' && buf[i + 1] == b'\n' {
            return Some(i + 2);
        }
        // \r\n\r\n boundary
        if i + 3 < buf.len()
            && buf[i..i + 4] == [b'\r', b'\n', b'\r', b'\n']
        {
            return Some(i + 4);
        }
        i += 1;
    }
    None
}

/// Parse the head block of one SSE frame into an `SseEvent`.
fn parse_event_lines(block: &[u8]) -> SseEvent {
    let text = String::from_utf8_lossy(block);
    let mut event = "message".to_string();
    let mut data = String::new();
    let mut first_data = true;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with(':') {
            continue; // comment line
        }
        if let Some(v) = line.strip_prefix("event:") {
            event = v.strip_prefix(' ').unwrap_or(v).to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            // W3C SSE: if value starts with a single space, remove it; preserve subsequent spaces (e.g. indentation)
            let val = v.strip_prefix(' ').unwrap_or(v);
            if first_data {
                data.push_str(val);
                first_data = false;
            } else {
                data.push('\n');
                data.push_str(val);
            }
        }
        // id / retry / other fields are ignored (not needed by the translators)
    }
    SseEvent { event, data }
}

/// Maximum buffered bytes for one SSE frame. A single legitimate delta frame
/// is at most a few KB; anything larger is a pathological upstream, and the
/// excess is shed to bound gateway memory instead of OOMing on it.
pub const MAX_SSE_FRAME_BYTES: usize = 64 * 1024;

/// Convert a byte stream into a stream of parsed SSE frames, reassembling
/// frames that are split across network chunks. Trailing bytes at EOF that
/// never formed a blank-line-terminated frame are discarded per SSE semantics
/// rather than emitted as a synthetic event.
pub fn sse_event_stream<S, E>(
    stream: S,
) -> impl Stream<Item = Result<SseEvent, E>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let buffer = BytesMut::new();
    let inner = Box::pin(stream);
    futures_util::stream::unfold((inner, buffer), |(mut st, mut buf)| async move {
        loop {
            if let Some(len) = find_sse_boundary(&buf) {
                let frame_block = buf.split_to(len);
                let evt = parse_event_lines(&frame_block);
                return Some((Ok(evt), (st, buf)));
            }
            if buf.len() > MAX_SSE_FRAME_BYTES {
                buf.clear();
            }
            match st.next().await {
                Some(Ok(bytes)) => buf.extend_from_slice(&bytes),
                Some(Err(e)) => return Some((Err(e), (st, buf))),
                None => return None,
            }
        }
    })
}

/// Serialize an Anthropic `MessageStreamEvent` as an Anthropic SSE frame
/// (`event: <type>\ndata: <json>\n\n`). Returns `None` for events that should
/// not be forwarded (e.g. `Unknown`).
pub fn anthropic_event_to_sse_bytes(event: &MessageStreamEvent) -> Option<Bytes> {
    let type_name = match event {
        MessageStreamEvent::MessageStart { .. } => "message_start",
        MessageStreamEvent::ContentBlockStart { .. } => "content_block_start",
        MessageStreamEvent::ContentBlockDelta { .. } => "content_block_delta",
        MessageStreamEvent::ContentBlockStop { .. } => "content_block_stop",
        MessageStreamEvent::MessageDelta { .. } => "message_delta",
        MessageStreamEvent::MessageStop => "message_stop",
        MessageStreamEvent::Ping => "ping",
        MessageStreamEvent::Error { .. } => "error",
        MessageStreamEvent::Unknown => return None,
    };
    let data = serde_json::to_string(event).ok()?;
    Some(Bytes::from(format!(
        "event: {}\ndata: {}\n\n",
        type_name, data
    )))
}

/// Serialize a Responses `ResponseStreamEvent` as an SSE frame
/// (`event: <type>\ndata: <json>\n\n`). Returns `None` for `Unknown`.
pub fn responses_event_to_sse_bytes(event: &ResponseStreamEvent) -> Option<Bytes> {
    let type_name = match event {
        ResponseStreamEvent::ResponseCreated { .. } => "response.created",
        ResponseStreamEvent::ResponseDone { .. } => "response.done",
        ResponseStreamEvent::OutputItemAdded { .. } => "response.output_item.added",
        ResponseStreamEvent::OutputItemDone { .. } => "response.output_item.done",
        ResponseStreamEvent::ContentPartAdded { .. } => "response.content_part.added",
        ResponseStreamEvent::ContentPartDone { .. } => "response.content_part.done",
        ResponseStreamEvent::TextDelta(_) => "response.text.delta",
        ResponseStreamEvent::OutputTextDelta(_) => "response.output_text.delta",
        ResponseStreamEvent::FunctionCallArgumentsDelta(_) => {
            "response.function_call_arguments.delta"
        }
        ResponseStreamEvent::Completed { .. } => "response.completed",
        ResponseStreamEvent::Incomplete { .. } => "response.incomplete",
        ResponseStreamEvent::Failed { .. } => "response.failed",
        ResponseStreamEvent::Unknown => return None,
    };
    let data = serde_json::to_string(event).ok()?;
    Some(Bytes::from(format!(
        "event: {}\ndata: {}\n\n",
        type_name, data
    )))
}

/// Translate an upstream **Responses** SSE byte stream into **OpenAI Chat** SSE
/// frames. Used by `/v1/chat/completions` with a Responses-native upstream.
///
/// Upstream `response.failed` handling: the protocol FSM projects the failure
/// (with its upstream code/message) into `Err(ProtocolError::Conversion)` and
/// latches itself done, so this bridge marks the stream stopped, pushes one
/// stream error item retaining `e.to_string()` (which `wrap_telemetry_stream`
/// records as `StreamFailed`), and emits no terminal stop chunk — the EOF tail
/// then sends only `data: [DONE]`.
///
/// Constraint: this branch runs after response headers are already committed,
/// so the gateway can no longer swap keys or fail over to another provider;
/// surfacing the failure mid-stream (telemetry + truncated stream) is the only
/// option, and retry relies on the client resending the request, which triggers
/// a fresh route.
///
/// Transport errors (`E`) and FSM failures are projected into one concrete
/// error type because the FSM failure carries no `E` value to forward.
#[derive(Debug)]
pub enum ResponsesChatStreamError {
    /// Upstream transport error detail (the `Display` of the original `E`).
    Transport(String),
    /// Upstream `response.failed` detail (`ProtocolError::Conversion` display,
    /// carrying the upstream code/message).
    UpstreamFailed(String),
}

impl std::fmt::Display for ResponsesChatStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResponsesChatStreamError::Transport(detail) => {
                write!(f, "upstream transport error: {}", detail)
            }
            ResponsesChatStreamError::UpstreamFailed(detail) => {
                write!(f, "upstream response failed: {}", detail)
            }
        }
    }
}

impl std::error::Error for ResponsesChatStreamError {}

pub fn responses_sse_to_chat_stream<S, E>(
    stream: S,
    fallback_model: &str,
) -> impl Stream<Item = Result<Bytes, ResponsesChatStreamError>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let fsm = std::sync::Arc::new(Mutex::new(ResponsesToChatFsm::new(fallback_model)));
    let fsm_flat = fsm.clone();
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped_flag = stopped.clone();

    let translated = sse_event_stream(stream).flat_map(move |res| {
        let mut out: Vec<Result<Bytes, ResponsesChatStreamError>> = Vec::new();
        match res {
            Ok(evt) => {
                let data = evt.data.trim();
                if data.is_empty() || data == "[DONE]" {
                    // terminal / heartbeat frame: nothing to forward
                } else if let Ok(msge) = serde_json::from_str::<ResponseStreamEvent>(data) {
                    // Upstream `response.failed` surfaces here as
                    // `Err(ProtocolError::Conversion)` carrying the upstream
                    // code/message: mark stopped, push one stream error item
                    // (never synthesize a stop chunk), EOF then sends only
                    // `data: [DONE]`.
                    match fsm_flat.lock().process_event(msge) {
                        Ok(chunks) => {
                            for c in chunks {
                                if c.choices.iter().any(|ch| ch.finish_reason.is_some()) {
                                    stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                                }
                                if let Ok(json) = serde_json::to_string(&c) {
                                    out.push(Ok(Bytes::from(format!("data: {}\n\n", json))));
                                }
                            }
                        }
                        Err(e) => {
                            stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            out.push(Err(ResponsesChatStreamError::UpstreamFailed(e.to_string())));
                        }
                    }
                }
            }
            Err(e) => {
                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                out.push(Err(ResponsesChatStreamError::Transport(e.to_string())));
            }
        }
        let iter = futures_util::stream::iter(out);
        futures_util::stream::BoxStream::from(Box::pin(iter)
            as std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, ResponsesChatStreamError>> + Send>>)
    });

    translated
        .chain(futures_util::stream::once(async move {
            let mut buf = Vec::new();
            if !stopped.load(std::sync::atomic::Ordering::SeqCst) {
                if let Some(chunk) = fsm.lock().finish_if_open() {
                    if let Ok(json) = serde_json::to_string(&chunk) {
                        buf.extend_from_slice(format!("data: {}\n\n", json).as_bytes());
                    }
                }
            }
            buf.extend_from_slice(b"data: [DONE]\n\n");
            Ok::<_, ResponsesChatStreamError>(Bytes::from(buf))
        }))
        .boxed()
}

/// Translate an upstream **OpenAI Chat** SSE byte stream into **Responses** SSE
/// frames. Used by `/v1/responses` with a Chat-native upstream.
pub fn chat_sse_to_responses_stream<S, E>(
    stream: S,
    fallback_model: &str,
) -> impl Stream<Item = Result<Bytes, E>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let fsm = std::sync::Arc::new(Mutex::new(ChatToResponsesFsm::new(fallback_model)));
    let fsm_flat = fsm.clone();
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped_flag = stopped.clone();

    let translated = sse_event_stream(stream).flat_map(move |res| {
        let mut out: Vec<Result<Bytes, E>> = Vec::new();
        match res {
            Ok(evt) => {
                let data = evt.data.trim();
                if data.is_empty() || data == "[DONE]" {
                    // terminal / heartbeat frame: nothing to forward
                } else if let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) {
                    if let Ok(events) = fsm_flat.lock().process_chunk(chunk) {
                        for e in events {
                            if matches!(
                                e,
                                ResponseStreamEvent::Completed { .. }
                                    | ResponseStreamEvent::Incomplete { .. }
                            ) {
                                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            }
                            if let Some(b) = responses_event_to_sse_bytes(&e) {
                                out.push(Ok(b));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                out.push(Err(e));
            }
        }
        let iter = futures_util::stream::iter(out);
        futures_util::stream::BoxStream::from(Box::pin(iter)
            as std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, E>> + Send>>)
    });

    translated
        .chain(futures_util::stream::once(async move {
            let synthetic = if !stopped.load(std::sync::atomic::Ordering::SeqCst) {
                match fsm.lock().finish_if_open().and_then(|e| responses_event_to_sse_bytes(&e)) {
                    Some(b) => b,
                    None => Bytes::new(),
                }
            } else {
                Bytes::new()
            };
            Ok::<_, E>(synthetic)
        }))
        .boxed()
}

/// Translate an upstream **Responses** SSE byte stream into **Anthropic** SSE
/// frames. Used by `/v1/messages` with a Responses-native upstream.
pub fn responses_sse_to_anthropic_stream<S, E>(
    stream: S,
    fallback_model: &str,
) -> impl Stream<Item = Result<Bytes, E>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let fsm = std::sync::Arc::new(Mutex::new(ResponsesToAnthropicFsm::new(fallback_model)));
    let fsm_flat = fsm.clone();
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped_flag = stopped.clone();

    let translated = sse_event_stream(stream).flat_map(move |res| {
        let mut out: Vec<Result<Bytes, E>> = Vec::new();
        match res {
            Ok(evt) => {
                if let Ok(msge) = serde_json::from_str::<ResponseStreamEvent>(&evt.data) {
                    if let Ok(events) = fsm_flat.lock().process_event(msge) {
                        for e in events {
                            if matches!(e, MessageStreamEvent::MessageStop) {
                                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            }
                            if let Some(b) = anthropic_event_to_sse_bytes(&e) {
                                out.push(Ok(b));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                out.push(Err(e));
            }
        }
        let iter = futures_util::stream::iter(out);
        futures_util::stream::BoxStream::from(Box::pin(iter)
            as std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, E>> + Send>>)
    });

    translated
        .chain(futures_util::stream::once(async move {
            let synthetic = if !stopped.load(std::sync::atomic::Ordering::SeqCst) {
                match fsm.lock().finish_if_open() {
                    Some(events) => {
                        let mut buf = Vec::new();
                        for e in &events {
                            if let Some(b) = anthropic_event_to_sse_bytes(e) {
                                buf.extend_from_slice(&b);
                            }
                        }
                        Bytes::from(buf)
                    }
                    None => Bytes::new(),
                }
            } else {
                Bytes::new()
            };
            Ok::<_, E>(synthetic)
        }))
        .boxed()
}

/// Translate an upstream **Anthropic** SSE byte stream into **Responses** SSE
/// frames. Used by `/v1/responses` with an Anthropic-native upstream.
pub fn anthropic_sse_to_responses_stream<S, E>(
    stream: S,
    fallback_model: &str,
) -> impl Stream<Item = Result<Bytes, E>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let fsm = std::sync::Arc::new(Mutex::new(AnthropicToResponsesFsm::new(fallback_model)));
    let fsm_flat = fsm.clone();
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped_flag = stopped.clone();

    let translated = sse_event_stream(stream).flat_map(move |res| {
        let mut out: Vec<Result<Bytes, E>> = Vec::new();
        match res {
            Ok(evt) => {
                if let Ok(msge) = serde_json::from_str::<MessageStreamEvent>(&evt.data) {
                    if let Ok(events) = fsm_flat.lock().process_event(msge) {
                        for e in events {
                            if matches!(
                                e,
                                ResponseStreamEvent::Completed { .. }
                                    | ResponseStreamEvent::Incomplete { .. }
                            ) {
                                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            }
                            if let Some(b) = responses_event_to_sse_bytes(&e) {
                                out.push(Ok(b));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                out.push(Err(e));
            }
        }
        let iter = futures_util::stream::iter(out);
        futures_util::stream::BoxStream::from(Box::pin(iter)
            as std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, E>> + Send>>)
    });

    translated
        .chain(futures_util::stream::once(async move {
            let synthetic = if !stopped.load(std::sync::atomic::Ordering::SeqCst) {
                match fsm.lock().finish_if_open().and_then(|e| responses_event_to_sse_bytes(&e)) {
                    Some(b) => b,
                    None => Bytes::new(),
                }
            } else {
                Bytes::new()
            };
            Ok::<_, E>(synthetic)
        }))
        .boxed()
}
pub fn openai_sse_to_anthropic_stream<S, E>(
    stream: S,
    fallback_model: &str,
) -> impl Stream<Item = Result<Bytes, E>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let fsm = std::sync::Arc::new(Mutex::new(ChatStreamToAnthropicFsm::new(fallback_model)));
    let fsm_flat = fsm.clone();
    // Track whether the FSM already emitted message_stop; if the upstream never
    // sends a finish_reason chunk, we synthesize the terminal events at EOF so
    // Anthropic clients never hang waiting for the message to conclude.
    // Transport errors latch `stopped` too so EOF never appends success frames.
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped_flag = stopped.clone();

    let translated = sse_event_stream(stream).flat_map(move |res| {
        let mut out: Vec<Result<Bytes, E>> = Vec::new();
        match res {
            Ok(evt) => {
                let data = evt.data.trim();
                if data.is_empty() || data == "[DONE]" {
                    // terminal / heartbeat frame: nothing to forward
                } else if let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) {
                    if let Ok(events) = fsm_flat.lock().process_chunk(chunk) {
                        for e in events {
                            if matches!(e, MessageStreamEvent::MessageStop) {
                                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            }
                            if let Some(b) = anthropic_event_to_sse_bytes(&e) {
                                out.push(Ok(b));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                out.push(Err(e));
            }
        }
        let iter = futures_util::stream::iter(out);
        futures_util::stream::BoxStream::from(Box::pin(iter)
            as std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, E>> + Send>>)
    });

    // At stream end, guarantee the Anthropic conversation terminates unless a
    // transport error already ended it with failure.
    translated
        .chain(futures_util::stream::once(async move {
            let synthetic = if !stopped.load(std::sync::atomic::Ordering::SeqCst) {
                match fsm.lock().finish_if_open() {
                    Some(events) => {
                        let mut buf = Vec::new();
                        for e in &events {
                            if let Some(b) = anthropic_event_to_sse_bytes(e) {
                                buf.extend_from_slice(&b);
                            }
                        }
                        Bytes::from(buf)
                    }
                    None => Bytes::new(),
                }
            } else {
                Bytes::new()
            };
            Ok::<_, E>(synthetic)
        }))
        .boxed()
}

/// Translate an upstream **Anthropic** SSE byte stream into **OpenAI** SSE
/// frames (`data: {chunk}\n\n`, terminating with `data: [DONE]`). Used by
/// `/v1/chat/completions` when the routed upstream is Anthropic-compatible.
pub fn anthropic_sse_to_openai_stream<S, E>(
    stream: S,
    fallback_model: &str,
) -> impl Stream<Item = Result<Bytes, E>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let fsm = std::sync::Arc::new(Mutex::new(AnthropicStreamToChatFsm::new(fallback_model)));
    let fsm_flat = fsm.clone();
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped_flag = stopped.clone();

    let translated = sse_event_stream(stream).flat_map(move |res| {
        let mut out: Vec<Result<Bytes, E>> = Vec::new();
        match res {
            Ok(evt) => {
                let data = evt.data.trim();
                if data.is_empty() || data == "[DONE]" {
                    // terminal / heartbeat frame: nothing to forward
                } else if let Ok(msge) = serde_json::from_str::<MessageStreamEvent>(data) {
                    if let Ok(chunks) = fsm_flat.lock().process_event(msge) {
                        for c in chunks {
                            if c.choices.iter().any(|ch| ch.finish_reason.is_some()) {
                                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            }
                            if let Ok(json) = serde_json::to_string(&c) {
                                out.push(Ok(Bytes::from(format!("data: {}\n\n", json))));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                out.push(Err(e));
            }
        }
        let iter = futures_util::stream::iter(out);
        futures_util::stream::BoxStream::from(Box::pin(iter)
            as std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, E>> + Send>>)
    });

    // OpenAI streams must terminate with `data: [DONE]`.
    translated
        .chain(futures_util::stream::once(async move {
            let mut buf = Vec::new();
            if !stopped.load(std::sync::atomic::Ordering::SeqCst) {
                if let Some(chunk) = fsm.lock().finish_if_open() {
                    if let Ok(json) = serde_json::to_string(&chunk) {
                        buf.extend_from_slice(format!("data: {}\n\n", json).as_bytes());
                    }
                }
            }
            buf.extend_from_slice(b"data: [DONE]\n\n");
            Ok::<_, E>(Bytes::from(buf))
        }))
        .boxed()
}

fn uuid_simple() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}", nanos)
}

/// Inspect an initial Antigravity candidate payload to see if it carries
/// any content (text, thought, or functionCall), or if it is an empty STOP.
pub fn is_antigravity_empty_stop_frame(val: &serde_json::Value) -> bool {
    let target = val.get("response").unwrap_or(val);
    let candidates = match target.get("candidates").and_then(|v| v.as_array()) {
        Some(c) => c,
        None => return false,
    };
    let first = match candidates.first() {
        Some(f) => f,
        None => return false,
    };

    let finish_reason = first.get("finishReason").and_then(|v| v.as_str());
    if finish_reason != Some("STOP") {
        return false;
    }

    // Check if there are any non-empty parts
    if let Some(parts) = first.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
        for p in parts {
            if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                if !t.is_empty() {
                    return false;
                }
            }
            if p.get("functionCall").is_some() {
                return false;
            }
        }
    }
    true
}

pub fn has_antigravity_content(val: &serde_json::Value) -> bool {
    let target = val.get("response").unwrap_or(val);
    let candidates = match target.get("candidates").and_then(|v| v.as_array()) {
        Some(c) => c,
        None => return false,
    };
    let first = match candidates.first() {
        Some(f) => f,
        None => return false,
    };
    if let Some(parts) = first.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
        for p in parts {
            if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                if !t.is_empty() {
                    return true;
                }
            }
            if p.get("functionCall").is_some() {
                return true;
            }
        }
    }
    false
}

/// Result of inspecting the initial preamble of an upstream Antigravity SSE stream.
pub enum AntigravityPreambleResult<S> {
    /// Preamble contains valid content or sufficient frames; ready to stream downstream.
    Ready {
        /// Buffered raw chunks received during preamble inspection.
        buffered: Vec<Bytes>,
        /// Live tail stream for remaining chunks.
        tail: S,
    },
    /// Upstream completed with finishReason: "STOP" and 0 content bytes across preamble frames.
    TransientEmptyStop {
        frames: usize,
    },
    /// Upstream safety block or deterministic error frame.
    DeterministicBlock {
        reason: String,
    },
    /// Stream ended prematurely before yielding any content or terminal candidate.
    AbruptTermination,
}

/// Default overall wall-clock budget for preamble verification. The verifier
/// only inspects frames until the first content frame, a terminal STOP, or this
/// deadline; keepalive-only warm-up phases (heartbeat pings) no longer force an
/// early `Ready` on a zero-content stream.
pub const DEFAULT_PREAMBLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// Minimum number of streaming attempts dedicated to absorbing upstream
/// transient empty-STOP completions. Empty STOPs fail fast in the preamble
/// (no downstream bytes committed, no key fault), so a dedicated budget larger
/// than the generic `max_retries` is cheap and credential-independent.
pub const MIN_EMPTY_STOP_ATTEMPTS: usize = 6;

/// Jittered exponential backoff before a transparent empty-STOP retry.
///
/// `attempt` is 1-based (the first retry waits ~250ms). Base doubles per
/// attempt, capped at 2s, with ±25% jitter derived from wall-clock nanos
/// (deliberately no `rand` dependency). Upstream empty-STOP episodes usually
/// last seconds; retrying instantly N times burns the whole budget inside the
/// outage window, which is exactly how `EMPTY_RESPONSE` used to reach clients.
pub fn empty_stop_retry_delay(attempt: usize) -> std::time::Duration {
    let shift = attempt.saturating_sub(1).min(4);
    let base_ms = 250u64.saturating_mul(1u64 << shift);
    let capped = base_ms.min(2000);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    // Jitter factor in [750, 1250] thousandths → [0.75x, 1.25x].
    let factor_milli = 750 + (nanos % 1000) * 500 / 1000;
    std::time::Duration::from_millis(capped * factor_milli / 1000)
}

/// Classify a collector error string as the upstream transient empty-STOP
/// anomaly. The marker substring is part of the contract asserted by tests
/// (`Antigravity stream completed with zero text and zero tool calls
/// (transient empty STOP)`), so routes can retry it transparently.
pub fn is_transient_empty_stop_error(msg: &str) -> bool {
    msg.contains("transient empty STOP")
}

/// Whether an upstream frame is "significant" for preamble frame accounting:
/// it either carries candidate content, a terminal finish reason, or a
/// deterministic block/error signal. Keepalive pings, usage-only frames and
/// role-only empty candidate frames are NOT significant and must not consume
/// the preamble frame budget (a burst of heartbeats during thinking warm-up
/// previously triggered `Ready` on a zero-content stream, committing the
/// response right before the late empty STOP arrived).
fn antigravity_frame_is_significant(val: &serde_json::Value) -> bool {
    let target = val.get("response").unwrap_or(val);
    if target
        .get("promptFeedback")
        .and_then(|f| f.get("blockReason"))
        .is_some()
    {
        return true;
    }
    if val.get("error").is_some() || target.get("error").is_some() {
        return true;
    }
    match target
        .get("candidates")
        .and_then(|v| v.as_array())
        .and_then(|c| c.first())
    {
        Some(first) => {
            if first.get("finishReason").is_some() {
                return true;
            }
            has_antigravity_content(val)
        }
        None => false,
    }
}

/// Bounded preamble verification for an Antigravity byte stream.
///
/// Inspects frames from the raw byte stream up to `max_frames` (default 8,
/// significant frames only) or until valid content (`text`, `thought`, or
/// `functionCall`) is confirmed. If an empty candidate with `finishReason == "STOP"`
/// is encountered before any content has been emitted, returns `TransientEmptyStop`
/// to allow transparent gateway-side retry.
pub async fn verify_antigravity_stream_preamble<S, E>(
    stream: S,
    chunk_timeout: std::time::Duration,
) -> Result<AntigravityPreambleResult<S>, E>
where
    S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
    E: Send + 'static,
{
    verify_antigravity_stream_preamble_with_deadline(
        stream,
        chunk_timeout,
        DEFAULT_PREAMBLE_DEADLINE,
    )
    .await
}

/// Like [`verify_antigravity_stream_preamble`], but with an explicit overall
/// wall-clock budget. When the budget lapses without content or terminal
/// signal, returns `Ready` with the buffered bytes (best-effort commit) so a
/// pathological upstream can never hold a request open indefinitely.
pub async fn verify_antigravity_stream_preamble_with_deadline<S, E>(
    stream: S,
    chunk_timeout: std::time::Duration,
    overall_deadline: std::time::Duration,
) -> Result<AntigravityPreambleResult<S>, E>
where
    S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
    E: Send + 'static,
{
    let mut inner = stream;
    let mut buffered_bytes: Vec<Bytes> = Vec::new();
    let mut frame_buf = BytesMut::new();
    let mut frames_inspected = 0;
    let max_frames = 8;
    let max_buffered_bytes = 64 * 1024;
    let deadline = tokio::time::Instant::now() + overall_deadline;

    loop {
        // First check if any full SSE event exists in the frame_buf
        while let Some(len) = find_sse_boundary(&frame_buf) {
            let frame_block = frame_buf.split_to(len);
            let evt = parse_event_lines(&frame_block);
            let data = evt.data.trim();

            if !data.is_empty() && data != "[DONE]" {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                    // Check for safety filter block
                    if let Some(feedback) = val.get("promptFeedback").or_else(|| val.get("response").and_then(|r| r.get("promptFeedback"))) {
                        if let Some(block_reason) = feedback.get("blockReason").and_then(|b| b.as_str()) {
                            return Ok(AntigravityPreambleResult::DeterministicBlock {
                                reason: format!("safety block: {}", block_reason),
                            });
                        }
                    }

                    if has_antigravity_content(&val) {
                        return Ok(AntigravityPreambleResult::Ready {
                            buffered: buffered_bytes,
                            tail: inner,
                        });
                    }

                    if is_antigravity_empty_stop_frame(&val) {
                        return Ok(AntigravityPreambleResult::TransientEmptyStop {
                            frames: frames_inspected,
                        });
                    }

                    // Only significant frames consume the frame budget.
                    if antigravity_frame_is_significant(&val) {
                        frames_inspected += 1;
                    }
                }
            } else if data == "[DONE]" {
                return Ok(AntigravityPreambleResult::TransientEmptyStop {
                    frames: frames_inspected,
                });
            }

            if frames_inspected >= max_frames {
                return Ok(AntigravityPreambleResult::Ready {
                    buffered: buffered_bytes,
                    tail: inner,
                });
            }
        }

        if buffered_bytes.iter().map(|b| b.len()).sum::<usize>() >= max_buffered_bytes {
            return Ok(AntigravityPreambleResult::Ready {
                buffered: buffered_bytes,
                tail: inner,
            });
        }

        // Fetch next chunk from upstream, bounded by both the per-chunk stall
        // timeout and the overall preamble deadline.
        let now = tokio::time::Instant::now();
        let Some(remaining) = deadline.checked_duration_since(now) else {
            return Ok(AntigravityPreambleResult::Ready {
                buffered: buffered_bytes,
                tail: inner,
            });
        };
        let deadline_bounded = remaining < chunk_timeout;
        match tokio::time::timeout(remaining.min(chunk_timeout), inner.next()).await {
            Ok(Some(Ok(bytes))) => {
                frame_buf.extend_from_slice(&bytes);
                buffered_bytes.push(bytes);
            }
            Ok(Some(Err(e))) => {
                return Err(e);
            }
            Ok(None) => {
                // Stream ended at EOF before seeing any content
                return Ok(AntigravityPreambleResult::TransientEmptyStop {
                    frames: frames_inspected,
                });
            }
            Err(_) => {
                if deadline_bounded {
                    // Overall preamble budget elapsed: commit what we have.
                    return Ok(AntigravityPreambleResult::Ready {
                        buffered: buffered_bytes,
                        tail: inner,
                    });
                }
                // Stalled chunk timeout
                return Ok(AntigravityPreambleResult::AbruptTermination);
            }
        }
    }
}

/// Translate an upstream **Antigravity** SSE byte stream into **OpenAI** SSE
/// frames (`data: {chunk}\n\n`, terminating with `data: [DONE]`). Used by
/// `/v1/chat/completions` when the routed upstream is Antigravity.
pub fn antigravity_sse_to_openai_stream<S, E>(
    stream: S,
    fallback_model: &str,
) -> impl Stream<Item = Result<Bytes, E>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let response_id = format!("chatcmpl-{}", uuid_simple());
    let model = fallback_model.to_string();
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped_flag = stopped.clone();
    let has_emitted_chunks = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let has_emitted_chunks_flag = has_emitted_chunks.clone();
    let transport_errored = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let transport_errored_flag = transport_errored.clone();

    let total_frames = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let total_frames_flag = total_frames.clone();
    let total_text_bytes = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let total_text_bytes_flag = total_text_bytes.clone();
    let total_thought_bytes = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let total_thought_bytes_flag = total_thought_bytes.clone();
    let total_tool_calls = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let total_tool_calls_flag = total_tool_calls.clone();
    let latest_finish_reason = std::sync::Arc::new(std::sync::RwLock::new(None::<String>));
    let latest_finish_reason_flag = latest_finish_reason.clone();
    let had_empty_stop_candidate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let had_empty_stop_candidate_flag = had_empty_stop_candidate.clone();

    let response_id_stream = response_id.clone();
    let model_stream = model.clone();

    tracing::debug!(
        model = %model,
        response_id = %response_id,
        "Initiating Antigravity SSE to OpenAI stream"
    );

    let translated = sse_event_stream(stream).flat_map(move |res| {
        let mut out: Vec<Result<Bytes, E>> = Vec::new();
        match res {
            Ok(evt) => {
                let data = evt.data.trim();
                if data.is_empty() || data == "[DONE]" {
                    // terminal / heartbeat frame: nothing to forward
                } else if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                    total_frames_flag.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    // Upstream error or safety block inspection
                    if let Some(err_obj) = val.get("error").or_else(|| val.get("response").and_then(|r| r.get("error"))) {
                        let err_msg = err_obj.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
                        let err_code = err_obj.get("code").map(|c| c.to_string()).unwrap_or_else(|| "none".to_string());
                        tracing::error!(
                            model = %model_stream,
                            response_id = %response_id_stream,
                            error_code = %err_code,
                            error_message = %err_msg,
                            "Antigravity upstream returned error frame during OpenAI SSE streaming"
                        );
                    }
                    if let Some(feedback) = val.get("promptFeedback").or_else(|| val.get("response").and_then(|r| r.get("promptFeedback"))) {
                        if let Some(block_reason) = feedback.get("blockReason").and_then(|b| b.as_str()) {
                            tracing::warn!(
                                model = %model_stream,
                                response_id = %response_id_stream,
                                block_reason = %block_reason,
                                "Antigravity prompt blocked by upstream safety/content filter"
                            );
                        }
                    }

                    if let Some(mut chunk) = antigravity_chunk_to_chat_chunk(&val, &model_stream, &response_id_stream) {
                        // Suppress raw empty STOP chunks when no content has ever been emitted.
                        // Upstream Google sometimes emits an initial frame with empty parts and finishReason STOP.
                        // Transmitting this chunk immediately signals a successful completion of zero length,
                        // triggering client EMPTY_RESPONSE assertions.
                        let is_empty_stop = chunk.choices.iter().all(|c| {
                            matches!(c.finish_reason, Some(ponyllm_protocol::openai::chat::FinishReason::Stop))
                                && c.delta.content.as_deref().unwrap_or("").is_empty()
                                && c.delta.reasoning_content.as_deref().unwrap_or("").is_empty()
                                && c.delta.tool_calls.is_none()
                        });

                        has_emitted_chunks_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        for ch in &chunk.choices {
                            if let Some(ref text) = ch.delta.content {
                                total_text_bytes_flag.fetch_add(text.len() as u64, std::sync::atomic::Ordering::Relaxed);
                            }
                            if let Some(ref reasoning) = ch.delta.reasoning_content {
                                total_thought_bytes_flag.fetch_add(reasoning.len() as u64, std::sync::atomic::Ordering::Relaxed);
                            }
                            if let Some(ref tc) = ch.delta.tool_calls {
                                total_tool_calls_flag.fetch_add(tc.len() as u64, std::sync::atomic::Ordering::Relaxed);
                            }
                        }

                        let had_tools = total_tool_calls_flag.load(std::sync::atomic::Ordering::Relaxed) > 0;
                        for ch in &mut chunk.choices {
                            // If this or any prior frame contained tool calls, sticky-override Stop to ToolCalls.
                            // Antigravity often outputs tool_calls in frame 1 without finishReason, then in frame 2
                            // emits empty parts with finishReason: "STOP". Downstream clients require finish_reason: "tool_calls".
                            if had_tools && matches!(ch.finish_reason, Some(ponyllm_protocol::openai::chat::FinishReason::Stop)) {
                                ch.finish_reason = Some(ponyllm_protocol::openai::chat::FinishReason::ToolCalls);
                            }
                            if let Some(ref fr) = ch.finish_reason {
                                let mut w = latest_finish_reason_flag.write().unwrap();
                                *w = Some(format!("{:?}", fr));
                            }
                        }

                        if chunk.choices.iter().any(|ch| ch.finish_reason.is_some()) {
                            tracing::debug!(
                                model = %model_stream,
                                response_id = %response_id_stream,
                                finish_reason = ?chunk.choices.first().and_then(|c| c.finish_reason.as_ref()),
                                "Antigravity SSE stream choice completed"
                            );
                            stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        }

                        let text_so_far = total_text_bytes_flag.load(std::sync::atomic::Ordering::Relaxed);
                        let tools_so_far = total_tool_calls_flag.load(std::sync::atomic::Ordering::Relaxed);
                        let thought_so_far = total_thought_bytes_flag.load(std::sync::atomic::Ordering::Relaxed);
                        // If this chunk is an empty STOP and zero content has been emitted so far,
                        // do NOT push the deceptive STOP chunk downstream!
                        if is_empty_stop && text_so_far == 0 && tools_so_far == 0 && thought_so_far == 0 {
                            had_empty_stop_candidate_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            tracing::warn!(
                                model = %model_stream,
                                response_id = %response_id_stream,
                                "Suppressing deceptive empty STOP chunk on zero-content stream"
                            );
                        } else if let Ok(json) = serde_json::to_string(&chunk) {
                            out.push(Ok(Bytes::from(format!("data: {}\n\n", json))));
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Antigravity SSE to OpenAI stream transport error encountered");
                transport_errored_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                out.push(Err(e));
            }
        }
        let iter = futures_util::stream::iter(out);
        futures_util::stream::BoxStream::from(Box::pin(iter)
            as std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, E>> + Send>>)
    });

    // OpenAI streams must terminate with `data: [DONE]`.
    translated
        .chain(futures_util::stream::once(async move {
            let mut buf = Vec::new();
            // If the stream did not stop normally and no transport error occurred,
            // synthesize the graceful Stop chunk ONLY if the stream actually emitted at least
            // one response chunk. If the upstream ended abruptly with zero chunks emitted,
            // never synthesize a fake Stop chunk that masks the empty termination.
            if !stopped.load(std::sync::atomic::Ordering::SeqCst)
                && !transport_errored.load(std::sync::atomic::Ordering::SeqCst)
                && has_emitted_chunks.load(std::sync::atomic::Ordering::SeqCst)
            {
                let had_tools = total_tool_calls.load(std::sync::atomic::Ordering::Relaxed) > 0;
                let synth_finish = if had_tools {
                    ponyllm_protocol::openai::chat::FinishReason::ToolCalls
                } else {
                    ponyllm_protocol::openai::chat::FinishReason::Stop
                };
                let final_chunk = ChatCompletionChunk {
                    id: response_id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    model: model.clone(),
                    choices: vec![ponyllm_protocol::openai::chat::ChatChunkChoice {
                        index: 0,
                        delta: ponyllm_protocol::openai::chat::ChatChunkDelta::default(),
                        finish_reason: Some(synth_finish),
                        logprobs: None,
                    }],
                    usage: None,
                    system_fingerprint: None,
                    service_tier: None,
                };
                if let Ok(json) = serde_json::to_string(&final_chunk) {
                    buf.extend_from_slice(format!("data: {}\n\n", json).as_bytes());
                }
            }

            let frames = total_frames.load(std::sync::atomic::Ordering::Relaxed);
            let text_bytes = total_text_bytes.load(std::sync::atomic::Ordering::Relaxed);
            let thought_bytes = total_thought_bytes.load(std::sync::atomic::Ordering::Relaxed);
            let tool_calls = total_tool_calls.load(std::sync::atomic::Ordering::Relaxed);
            let finish_reason = latest_finish_reason.read().unwrap().clone();

            if text_bytes == 0 && tool_calls == 0 && !transport_errored.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::warn!(
                    model = %model,
                    response_id = %response_id,
                    total_frames = frames,
                    thought_bytes = thought_bytes,
                    text_bytes = 0,
                    tool_calls = 0,
                    finish_reason = ?finish_reason,
                    "Antigravity SSE to OpenAI stream finalized with ZERO content bytes! Emitting stream error event to guide client retry."
                );
                // Only if the stream actually had an empty STOP candidate,
                // emit the EMPTY_RESPONSE error frame to notify downstream of the failure.
                if had_empty_stop_candidate.load(std::sync::atomic::Ordering::SeqCst) {
                    let err_payload = serde_json::json!({
                        "error": {
                            "message": format!("model \"{}\" returned a completed response with no content (upstream transient empty STOP)", model),
                            "type": "server_error",
                            "param": null,
                            "code": "EMPTY_RESPONSE"
                        }
                    });
                    if let Ok(err_str) = serde_json::to_string(&err_payload) {
                        buf.extend_from_slice(format!("data: {}\n\n", err_str).as_bytes());
                    }
                }
            } else {
                tracing::debug!(
                    response_id = %response_id,
                    model = %model,
                    total_frames = frames,
                    text_bytes = text_bytes,
                    thought_bytes = thought_bytes,
                    tool_calls = tool_calls,
                    finish_reason = ?finish_reason,
                    "Antigravity SSE to OpenAI stream finalized with [DONE]"
                );
            }
            buf.extend_from_slice(b"data: [DONE]\n\n");
            Ok::<_, E>(Bytes::from(buf))
        }))
        .boxed()
}

/// Translate an upstream **Antigravity** SSE byte stream into **Anthropic** SSE
/// frames (`event: <type>\ndata: <json>\n\n`). Used by `/v1/messages` when
/// the routed upstream is Antigravity.
pub fn antigravity_sse_to_anthropic_stream<S, E>(
    stream: S,
    fallback_model: &str,
) -> impl Stream<Item = Result<Bytes, E>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let fsm = std::sync::Arc::new(Mutex::new(ChatStreamToAnthropicFsm::new(fallback_model)));
    let fsm_flat = fsm.clone();
    let response_id = format!("chatcmpl-{}", uuid_simple());
    let model = fallback_model.to_string();
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped_flag = stopped.clone();

    let total_frames = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let total_frames_flag = total_frames.clone();
    let total_content_events = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let total_content_events_flag = total_content_events.clone();
    let has_tool_calls = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let has_tool_calls_flag = has_tool_calls.clone();
    let transport_errored = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let transport_errored_flag = transport_errored.clone();

    let response_id_stream = response_id.clone();
    let model_stream = model.clone();

    tracing::debug!(
        model = %model,
        response_id = %response_id,
        "Initiating Antigravity SSE to Anthropic stream"
    );

    let translated = sse_event_stream(stream).flat_map(move |res| {
        let mut out: Vec<Result<Bytes, E>> = Vec::new();
        match res {
            Ok(evt) => {
                let data = evt.data.trim();
                if data.is_empty() || data == "[DONE]" {
                    // terminal / heartbeat frame: nothing to forward
                } else if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                    total_frames_flag.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    // Upstream error or safety block inspection
                    if let Some(err_obj) = val.get("error").or_else(|| val.get("response").and_then(|r| r.get("error"))) {
                        let err_msg = err_obj.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
                        let err_code = err_obj.get("code").map(|c| c.to_string()).unwrap_or_else(|| "none".to_string());
                        tracing::error!(
                            model = %model_stream,
                            response_id = %response_id_stream,
                            error_code = %err_code,
                            error_message = %err_msg,
                            "Antigravity upstream returned error frame during Anthropic SSE streaming"
                        );
                    }
                    if let Some(feedback) = val.get("promptFeedback").or_else(|| val.get("response").and_then(|r| r.get("promptFeedback"))) {
                        if let Some(block_reason) = feedback.get("blockReason").and_then(|b| b.as_str()) {
                            tracing::warn!(
                                model = %model_stream,
                                response_id = %response_id_stream,
                                block_reason = %block_reason,
                                "Antigravity prompt blocked by upstream safety/content filter"
                            );
                        }
                    }

                    if let Some(mut chunk) = antigravity_chunk_to_chat_chunk(&val, &model_stream, &response_id_stream) {
                        if chunk.choices.iter().any(|c| c.delta.tool_calls.is_some()) {
                            has_tool_calls_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                        let had_tools = has_tool_calls_flag.load(std::sync::atomic::Ordering::SeqCst);
                        for ch in &mut chunk.choices {
                            if had_tools && matches!(ch.finish_reason, Some(ponyllm_protocol::openai::chat::FinishReason::Stop)) {
                                ch.finish_reason = Some(ponyllm_protocol::openai::chat::FinishReason::ToolCalls);
                            }
                        }
                        if let Ok(events) = fsm_flat.lock().process_chunk(chunk) {
                            for e in events {
                                if matches!(
                                    e,
                                    MessageStreamEvent::ContentBlockDelta { .. }
                                        | MessageStreamEvent::ContentBlockStart { .. }
                                ) {
                                    total_content_events_flag.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                }
                                if matches!(e, MessageStreamEvent::MessageStop) {
                                    tracing::debug!(
                                        model = %model_stream,
                                        response_id = %response_id_stream,
                                        "Antigravity SSE to Anthropic stream reached MessageStop"
                                    );
                                    stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                                }
                                if let Some(b) = anthropic_event_to_sse_bytes(&e) {
                                    out.push(Ok(b));
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Antigravity SSE to Anthropic stream transport error encountered");
                transport_errored_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                out.push(Err(e));
            }
        }
        let iter = futures_util::stream::iter(out);
        futures_util::stream::BoxStream::from(Box::pin(iter)
            as std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, E>> + Send>>)
    });

    // At stream end, guarantee the Anthropic conversation terminates unless a
    // transport error already ended it with failure.
    let term_response_id = response_id.clone();
    let term_model = fallback_model.to_string();
    translated
        .chain(futures_util::stream::once(async move {
            let synthetic = if !stopped.load(std::sync::atomic::Ordering::SeqCst) {
                match fsm.lock().finish_if_open() {
                    Some(events) => {
                        let mut buf = Vec::new();
                        for e in &events {
                            if let Some(b) = anthropic_event_to_sse_bytes(e) {
                                buf.extend_from_slice(&b);
                            }
                        }
                        Bytes::from(buf)
                    }
                    None => Bytes::new(),
                }
            } else {
                Bytes::new()
            };

            let frames = total_frames.load(std::sync::atomic::Ordering::Relaxed);
            let content_events = total_content_events.load(std::sync::atomic::Ordering::Relaxed);

            if content_events == 0 && !transport_errored.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::warn!(
                    response_id = %term_response_id,
                    model = %term_model,
                    total_frames = frames,
                    content_events = 0,
                    "Antigravity SSE to Anthropic stream finalized with ZERO content events! Emitting stream error event to guide client retry."
                );
                let err_event = MessageStreamEvent::Error {
                    error: ponyllm_protocol::anthropic::AnthropicErrorDetail {
                        r#type: "api_error".to_string(),
                        message: format!("model \"{}\" returned a completed response with no content (upstream transient empty STOP)", term_model),
                    },
                };
                let mut buf = Vec::new();
                if let Some(b) = anthropic_event_to_sse_bytes(&err_event) {
                    buf.extend_from_slice(&b);
                }
                Ok::<_, E>(Bytes::from(buf))
            } else {
                tracing::debug!(
                    response_id = %term_response_id,
                    model = %term_model,
                    total_frames = frames,
                    content_events = content_events,
                    "Antigravity SSE to Anthropic stream finalized"
                );
                Ok::<_, E>(synthetic)
            }
        }))
        .boxed()
}

/// Collect upstream Antigravity SSE stream into a consolidated Gemini response Value.
pub async fn collect_antigravity_sse_to_json<S, E>(stream: S) -> Result<serde_json::Value, String>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    collect_antigravity_sse_to_json_with_timeout(stream, std::time::Duration::from_secs(15)).await
}

/// Collect upstream Antigravity SSE stream into a consolidated Gemini response Value with configurable chunk timeout.
pub async fn collect_antigravity_sse_to_json_with_timeout<S, E>(
    stream: S,
    chunk_timeout: std::time::Duration,
) -> Result<serde_json::Value, String>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let mut sse_stream = Box::pin(sse_event_stream(stream));
    let mut collected_parts: Vec<serde_json::Value> = Vec::new();
    let mut finish_reason = None;
    let mut usage_metadata = serde_json::json!({
        "promptTokenCount": 0,
        "candidatesTokenCount": 0,
        "totalTokenCount": 0
    });
    let mut has_data = false;
    let mut frame_count: u32 = 0;
    let mut total_thought_bytes: usize = 0;
    let mut total_text_bytes: usize = 0;
    let mut function_call_count: usize = 0;

    loop {
        let chunk_res = match tokio::time::timeout(chunk_timeout, sse_stream.next()).await {
            Ok(Some(res)) => res,
            Ok(None) => break,
            Err(_) => return Err(format!("Antigravity SSE stream stalled: {:?} chunk timeout exceeded", chunk_timeout)),
        };

        match chunk_res {
            Ok(evt) => {
                let data = evt.data.trim();
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(err_obj) = val.get("error").or_else(|| val.get("response").and_then(|r| r.get("error"))) {
                        let msg = err_obj.get("message").and_then(|m| m.as_str()).unwrap_or("Antigravity upstream error frame");
                        tracing::error!(frame = frame_count + 1, error = %msg, "Antigravity upstream returned error frame");
                        return Err(format!("Antigravity stream error frame: {}", msg));
                    }
                    has_data = true;
                    frame_count += 1;
                    let target = val.get("response").unwrap_or(&val);
                    if let Some(candidates) = target.get("candidates").and_then(|v| v.as_array()) {
                        if let Some(cand) = candidates.first() {
                            if let Some(fr) = cand.get("finishReason").and_then(|f| f.as_str()) {
                                finish_reason = Some(fr.to_string());
                                tracing::debug!(frame = frame_count, finish_reason = %fr, "Antigravity SSE frame carries finishReason");
                            }
                            if let Some(parts) = cand.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                                for part in parts {
                                    tracing::info!("Antigravity candidate part: {:?}", part);
                                    let is_thought = part.get("thought").and_then(|t| t.as_bool()).unwrap_or(false);
                                    if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                        if is_thought {
                                            total_thought_bytes += txt.len();
                                        } else {
                                            total_text_bytes += txt.len();
                                        }
                                    }
                                    if part.get("functionCall").is_some() {
                                        function_call_count += 1;
                                    }
                                    if let (Some(last), Some(new_text)) = (collected_parts.last_mut(), part.get("text").and_then(|t| t.as_str())) {
                                        let last_thought = last.get("thought").and_then(|t| t.as_bool()).unwrap_or(false);
                                        let new_thought = part.get("thought").and_then(|t| t.as_bool()).unwrap_or(false);
                                        if last.get("text").is_some() && last_thought == new_thought {
                                            if let Some(old_text) = last.get_mut("text") {
                                                if let Some(s) = old_text.as_str() {
                                                    *old_text = serde_json::Value::String(format!("{}{}", s, new_text));
                                                    continue;
                                                }
                                            }
                                        }
                                    }
                                    collected_parts.push(part.clone());
                                }
                            }
                        }
                    }
                    if let Some(usage) = target.get("usageMetadata") {
                        usage_metadata = usage.clone();
                    }
                    tracing::trace!(
                        frame = frame_count,
                        parts_in_frame = target.get("candidates").and_then(|c| c.as_array()).and_then(|c| c.first()).and_then(|f| f.get("content")).and_then(|c| c.get("parts")).and_then(|p| p.as_array()).map(|p| p.len()).unwrap_or(0),
                        "Processed Antigravity SSE frame"
                    );
                }
            }
            Err(e) => {
                return Err(format!("SSE stream error while collecting: {}", e));
            }
        }
    }

    if !has_data && collected_parts.is_empty() {
        return Err("No data collected from Antigravity SSE stream".to_string());
    }

    let final_finish_reason = finish_reason.unwrap_or_else(|| "STOP".to_string());

    if total_text_bytes == 0 && function_call_count == 0 {
        tracing::warn!(
            total_frames = frame_count,
            total_thought_bytes,
            total_text_bytes = 0,
            function_call_count,
            finish_reason = %final_finish_reason,
            usage = ?usage_metadata,
            "Antigravity SSE stream completed with ZERO content bytes! (Model produced only thoughts or hit max_tokens/safety stop)"
        );
        // Transparent Gateway Retry: If upstream terminated with STOP but produced zero
        // text and zero tool calls, this is a transient upstream anomaly (empty completion).
        // Returning an error here triggers the gateway's automatic failover/retry loop,
        // preventing downstream clients (Claude Code, Codex, pi-ai) from receiving an empty completion.
        if final_finish_reason == "STOP" {
            return Err("Antigravity stream completed with zero text and zero tool calls (transient empty STOP)".to_string());
        }
    } else {
        tracing::debug!(
            total_frames = frame_count,
            total_thought_bytes,
            total_text_bytes,
            function_call_count,
            finish_reason = %final_finish_reason,
            usage = ?usage_metadata,
            "Antigravity SSE stream collection finished successfully"
        );
    }

    Ok(serde_json::json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": collected_parts
            },
            "finishReason": final_finish_reason
        }],
        "usageMetadata": usage_metadata
    }))
}

/// Context for single-append stream telemetry.
///
/// Streaming routes emit a `StreamStarted` event when the upstream connection
/// is established. Completion, mid-stream failures and client cancels are
/// appended with the same `request_id`; metrics and frames derive from them.
#[derive(Debug, Clone)]
pub struct StreamFailureContext {
    pub bus: Arc<EventBus>,
    pub ctx: EventCtx,
    pub provider: String,
    pub stages: Arc<Mutex<StageTimings>>,
    pub request_snippet: Option<String>,
    pub estimated_prompt_tokens: u64,
    pub attempt_start: Option<Instant>,
}

/// Telemetry wrapper stream tracking TTFT on first emitted chunk and measuring TPS on completion.
pub struct TelemetryStream<S> {
    inner: S,
    failure_ctx: StreamFailureContext,
    first_token_time: Option<Instant>,
    last_chunk_time: Option<Instant>,
    gaps_ms: Vec<f64>,
    max_gap_ms: f64,
    stall_count: u64,
    chunks_emitted: u64,
    bytes_emitted: u64,
    content_chars_emitted: u64,
    usage_completion_tokens: Option<u64>,
    usage_prompt_tokens: Option<u64>,
    usage_cached_tokens: Option<u64>,
    collected_text: String,
    has_error: bool,
    completed: bool,
    last_error: Option<String>,
}

const STALL_GAP_MS: f64 = 1000.0;
/// Emit a `StreamProgress` event every N chunks: O(1) amortized observability
/// for long streams without per-chunk log volume.
const PROGRESS_EVERY: u64 = 64;

impl<S> TelemetryStream<S> {
    pub fn new(inner: S, failure_ctx: StreamFailureContext) -> Self {
        Self {
            inner,
            failure_ctx,
            first_token_time: None,
            last_chunk_time: None,
            gaps_ms: Vec::new(),
            max_gap_ms: 0.0,
            stall_count: 0,
            chunks_emitted: 0,
            bytes_emitted: 0,
            content_chars_emitted: 0,
            usage_completion_tokens: None,
            usage_prompt_tokens: None,
            usage_cached_tokens: None,
            collected_text: String::new(),
            has_error: false,
            completed: false,
            last_error: None,
        }
    }

    fn observe_chunk(&mut self, item_bytes: &[u8]) {
        let now = Instant::now();
        if let Some(last) = self.last_chunk_time {
            let gap = now.saturating_duration_since(last).as_secs_f64() * 1000.0;
            self.gaps_ms.push(gap);
            if gap > self.max_gap_ms {
                self.max_gap_ms = gap;
            }
            if gap >= STALL_GAP_MS {
                self.stall_count += 1;
            }
        } else {
            self.first_token_time = Some(now);
        }
        self.last_chunk_time = Some(now);
        self.chunks_emitted += 1;
        self.bytes_emitted += item_bytes.len() as u64;

        // Inspect SSE payload to extract real content characters and usage output_tokens
        let (chars, usage_comp, usage_prompt, usage_cached, text_delta) = estimate_tokens_from_sse_bytes(item_bytes);
        self.content_chars_emitted += chars as u64;
        if !text_delta.is_empty() && self.collected_text.len() < 10 * 1024 * 1024 {
            self.collected_text.push_str(&text_delta);
        }
        if let Some(toks) = usage_comp {
            self.usage_completion_tokens = Some(toks);
        }
        if let Some(toks) = usage_prompt {
            self.usage_prompt_tokens = Some(toks);
        }
        if let Some(toks) = usage_cached {
            self.usage_cached_tokens = Some(toks);
        }
    }

    fn build_flow(&self, now: Instant) -> (StreamFlowSample, Option<f64>) {
        let start = self.failure_ctx.ctx.start;
        let attempt_start = self.failure_ctx.attempt_start.unwrap_or(start);
        
        // Pure upstream TTFT: from successful attempt dispatch to first chunk received
        let upstream_ttft_ms = self.first_token_time.map(|t| {
            (t.saturating_duration_since(attempt_start).as_secs_f64() * 1000.0).max(1.0)
        });
        // End-to-end downstream TTFT: from client request start to first chunk yielded
        let downstream_ttft_ms = self.first_token_time.map(|t| {
            (t.saturating_duration_since(start).as_secs_f64() * 1000.0).max(1.0)
        });
        let ttlb_ms = now.saturating_duration_since(start).as_secs_f64() * 1000.0;

        // Accurate token calculation:
        // 1. If upstream emitted an explicit usage token count in terminal frame, strictly use it.
        // 2. Otherwise estimate based on actual content characters (approx 3.5 chars / token, bounded below by chunks).
        // 3. Never use raw SSE wire protocol bytes (which include hundreds of bytes of JSON boilerplate per chunk).
        let completion_tokens = if let Some(toks) = self.usage_completion_tokens {
            toks.max(1)
        } else if self.content_chars_emitted > 0 {
            // Standard NLP heuristic: ~3.5 chars per English token, ~1-2 chars per CJK token.
            // (content_chars / 3.0) bounded by chunks_emitted gives an accurate estimate.
            let est_tokens = (self.content_chars_emitted as f64 / 3.0).ceil() as u64;
            est_tokens.max(self.chunks_emitted.min(1))
        } else {
            // Pure control / thought frames or empty payload: fallback to chunks count
            self.chunks_emitted.max(1)
        };

        let tps = if let Some(ft) = self.first_token_time {
            let gen_dur = now.saturating_duration_since(ft).as_secs_f64();
            if gen_dur > 0.05 && completion_tokens > 0 {
                Some((completion_tokens as f64 / gen_dur).max(1.0))
            } else {
                None
            }
        } else {
            None
        };
        let (p50, p95, max) = gap_percentiles(self.gaps_ms.clone());
        let max_gap = if self.max_gap_ms > 0.0 { Some(self.max_gap_ms) } else { max };
        let avg_gap = if self.gaps_ms.is_empty() {
            None
        } else {
            Some(self.gaps_ms.iter().sum::<f64>() / self.gaps_ms.len() as f64)
        };
        let sample = StreamFlowSample {
            ttft_ms: upstream_ttft_ms,
            downstream_ttft_ms,
            ttlb_ms,
            chunks: self.chunks_emitted,
            bytes: self.bytes_emitted,
            max_gap_ms: max_gap,
            stall_count: self.stall_count,
            tps,
            tpot_p50_ms: p50,
            tpot_p95_ms: p95,
            tpot_mean_ms: avg_gap,
            prompt_tokens: self.usage_prompt_tokens.unwrap_or(self.failure_ctx.estimated_prompt_tokens),
            completion_tokens,
            cached_tokens: self.usage_cached_tokens.unwrap_or(0),
        };
        (sample, avg_gap)
    }

    fn emit(&self, provider: Option<String>, event: GatewayEvent) {
        let fctx = &self.failure_ctx;
        fctx.bus.append(&fctx.ctx, provider.or(Some(fctx.provider.clone())), event);
    }

    fn finish_stages(&self, upstream_ttft: Option<f64>, downstream_ttft: Option<f64>) -> StageTimings {
        let mut stages = self.failure_ctx.stages.lock().clone();
        if stages.upstream_ttft_ms.is_none() || upstream_ttft.is_some() {
            stages.upstream_ttft_ms = upstream_ttft;
        }
        stages.downstream_ttft_ms = downstream_ttft;
        stages
    }

    fn emit_failure(&self, reason: &str, flow: Option<StreamFlowSample>, stages: StageTimings) {
        let error = self
            .last_error
            .clone()
            .unwrap_or_else(|| reason.to_string());
        self.emit(
            None,
            GatewayEvent::StreamFailed {
                error,
                flow,
                stages,
                request_snippet: self.failure_ctx.request_snippet.clone(),
            },
        );
    }
}

impl<S, E> Stream for TelemetryStream<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    type Item = Result<Bytes, E>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let res = std::pin::Pin::new(&mut self.inner).poll_next(cx);
        match res {
            std::task::Poll::Ready(Some(Ok(item))) => {
                self.observe_chunk(item.as_ref());
                if self.chunks_emitted.is_multiple_of(PROGRESS_EVERY) {
                    self.emit(
                        None,
                        GatewayEvent::StreamProgress {
                            chunks: self.chunks_emitted,
                            bytes: self.bytes_emitted,
                        },
                    );
                }
                std::task::Poll::Ready(Some(Ok(item)))
            }
            std::task::Poll::Ready(Some(Err(e))) => {
                self.has_error = true;
                self.last_error = Some(e.to_string());
                std::task::Poll::Ready(Some(Err(e)))
            }
            std::task::Poll::Ready(None) => {
                if !self.completed {
                    self.completed = true;
                    let now = Instant::now();
                    let (sample, _avg_gap) = self.build_flow(now);
                    let stages = self.finish_stages(sample.ttft_ms, sample.downstream_ttft_ms);
                    if self.has_error {
                        self.emit_failure("stream terminated with error", Some(sample), stages);
                    } else {
                        let resp_snippet = if !self.collected_text.is_empty() {
                            Some(self.collected_text.clone())
                        } else {
                            None
                        };
                        self.emit(
                            None,
                            GatewayEvent::StreamCompleted {
                                flow: sample,
                                stages,
                                request_snippet: self
                                    .failure_ctx
                                    .request_snippet
                                    .clone(),
                                response_snippet: resp_snippet,
                            },
                        );
                    }
                }
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl<S> Drop for TelemetryStream<S> {
    fn drop(&mut self) {
        if !self.completed {
            self.completed = true;
            let now = Instant::now();

            // Record only genuine failures, not client-side cancellations:
            // - transport error seen before drop, or
            // - stream died before the first chunk (never reached TTFT).
            // A client disconnect after chunks flowed is a cancel, not an error.
            if self.has_error || self.chunks_emitted == 0 {
                let (sample, _avg_gap) = self.build_flow(now);
                let stages = self.finish_stages(sample.ttft_ms, sample.downstream_ttft_ms);
                self.emit_failure("stream dropped before completion", Some(sample), stages);
            } else {
                let ttlb_ms = now
                    .saturating_duration_since(self.failure_ctx.ctx.start)
                    .as_secs_f64()
                    * 1000.0;
                self.emit(
                    None,
                    GatewayEvent::StreamCancelled {
                        chunks: self.chunks_emitted,
                        bytes: self.bytes_emitted,
                        ttlb_ms,
                    },
                );
            }
        }
    }
}

/// Quick extraction of real content characters and usage tokens from SSE chunk bytes.
/// Inspects `data:` lines for delta text and usage objects to prevent wire JSON boilerplate
/// from inflating completion token counts and distorting TPS.
fn estimate_tokens_from_sse_bytes(raw: &[u8]) -> (usize, Option<u64>, Option<u64>, Option<u64>, String) {
    let Ok(text) = std::str::from_utf8(raw) else {
        return (0, None, None, None, String::new());
    };

    let mut content_chars = 0;
    let mut usage_completion = None;
    let mut usage_prompt = None;
    let mut usage_cached = None;
    let mut text_delta = String::new();

    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line).trim();
        let Some(data_str) = line.strip_prefix("data:") else {
            continue;
        };
        let data_str = data_str.trim();
        if data_str.is_empty() || data_str == "[DONE]" {
            continue;
        }

        let Ok(val) = serde_json::from_str::<serde_json::Value>(data_str) else {
            continue;
        };

        if let Some(obj) = val.as_object() {
            // 1. Inspect usage if present
            let (prompt, completion, cached) = extract_usage_tokens(&val);
            if prompt > 0 || completion > 0 || cached > 0 {
                usage_prompt = Some(prompt);
                usage_completion = Some(completion);
                usage_cached = Some(cached);
            }

            // 2. OpenAI Choices delta
            if let Some(choices) = obj.get("choices").and_then(|c| c.as_array()) {
                for ch in choices {
                    if let Some(delta) = ch.get("delta").and_then(|d| d.as_object()) {
                        if let Some(c) = delta.get("content").and_then(|s| s.as_str()) {
                            content_chars += c.len();
                            text_delta.push_str(c);
                        }
                        if let Some(rc) = delta.get("reasoning_content").and_then(|s| s.as_str()) {
                            content_chars += rc.len();
                            text_delta.push_str(rc);
                        }
                    }
                }
            }

            // 3. Anthropic ContentBlockDelta / MessageDelta
            if let Some(delta) = obj.get("delta").and_then(|d| d.as_object()) {
                if let Some(t) = delta.get("text").or_else(|| delta.get("thinking")).and_then(|s| s.as_str()) {
                    content_chars += t.len();
                    text_delta.push_str(t);
                }
            }

            // 4. Responses output_text.delta
            if let Some(d) = obj.get("delta").and_then(|s| s.as_str()) {
                content_chars += d.len();
                text_delta.push_str(d);
            }
        }
    }

    (content_chars, usage_completion, usage_prompt, usage_cached, text_delta)
}

pub fn wrap_telemetry_stream<S, E>(
    stream: S,
    failure_ctx: StreamFailureContext,
) -> impl Stream<Item = Result<Bytes, E>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
    E: Send + std::fmt::Display + 'static,
{
    TelemetryStream::new(stream, failure_ctx)
}

fn parse_lenient_u64(v: &serde_json::Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_f64().map(|f| f as u64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Extract prompt_tokens, completion_tokens, and cached_tokens from OpenAI/Anthropic/Antigravity JSON usage object
pub fn extract_usage_tokens(val: &serde_json::Value) -> (u64, u64, u64) {
    let usage_opt = val.get("usage")
        .or_else(|| val.get("response").and_then(|r| r.get("usage")))
        .or_else(|| val.get("message").and_then(|m| m.get("usage")))
        .or_else(|| val.get("usageMetadata"));

    if let Some(usage) = usage_opt {
        let (prompt, cached) = if let Some(p) = usage.get("prompt_tokens").and_then(parse_lenient_u64) {
            let cached = usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .or_else(|| usage.get("cached_tokens"))
                .or_else(|| usage.get("prompt_cache_hit_tokens"))
                .and_then(parse_lenient_u64)
                .unwrap_or(0);
            (p, cached)
        } else if let Some(input) = usage.get("input_tokens").and_then(parse_lenient_u64) {
            // Anthropic 兼容处理：总 Prompt = input + cache_read + cache_creation
            let cached_read = usage.get("cache_read_input_tokens").and_then(parse_lenient_u64).unwrap_or(0);
            let cached_create = usage.get("cache_creation_input_tokens").and_then(parse_lenient_u64).unwrap_or(0);
            let total_prompt = input.saturating_add(cached_read).saturating_add(cached_create);
            (total_prompt, cached_read)
        } else if let Some(ptc) = usage.get("promptTokenCount").and_then(parse_lenient_u64) {
            // Gemini / Antigravity
            let cached = usage.get("cachedContentTokenCount").and_then(parse_lenient_u64).unwrap_or(0);
            (ptc, cached)
        } else {
            (0, 0)
        };

        let completion = usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .or_else(|| usage.get("candidatesTokenCount"))
            .and_then(parse_lenient_u64)
            .unwrap_or(0);

        (prompt, completion, cached)
    } else {
        (0, 0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use ponyllm_core::telemetry::{FlightRecorder, GatewayEvent, MetricsCollector};

    fn bytes_stream(chunks: Vec<Bytes>) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
        futures_util::stream::iter(chunks.into_iter().map(Ok::<Bytes, std::io::Error>))
    }

    #[tokio::test]
    async fn test_extract_event_split_across_chunks() {        let s = bytes_stream(vec![
            Bytes::from_static(b"data: {\"a\":1}\n"),
            Bytes::from_static(b"\ndata: {\"b\":2}\n\n"),
        ]);
        let events: Vec<SseEvent> = sse_event_stream(s)
            .map(|r| r.unwrap())
            .collect()
            .await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "{\"a\":1}");
        assert_eq!(events[0].event, "message");
        assert_eq!(events[1].data, "{\"b\":2}");
    }

    #[tokio::test]
    async fn test_extract_event_crlf() {
        let s = bytes_stream(vec![Bytes::from_static(
            b"event: message_start\r\ndata: {\"x\":1}\r\n\r\n",
        )]);
        let events: Vec<SseEvent> = sse_event_stream(s)
            .map(|r| r.unwrap())
            .collect()
            .await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "message_start");
        assert_eq!(events[0].data, "{\"x\":1}");
    }

    #[tokio::test]
    async fn test_multiple_events_in_single_chunk() {
        let chunk = Bytes::from_static(b"data: first\n\ndata: second\n\nevent: custom\ndata: third\n\n");
        let s = bytes_stream(vec![chunk]);
        let events: Vec<SseEvent> = sse_event_stream(s)
            .map(|r| r.unwrap())
            .collect()
            .await;
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].data, "first");
        assert_eq!(events[1].data, "second");
        assert_eq!(events[2].event, "custom");
        assert_eq!(events[2].data, "third");
    }

    #[tokio::test]
    async fn test_parse_multiline_data_preserves_indentation() {
        // According to W3C SSE, only the first space after 'data:' is stripped.
        let s = bytes_stream(vec![Bytes::from_static(b"data:    def foo():\n\n")]);
        let events: Vec<SseEvent> = sse_event_stream(s)
            .map(|r| r.unwrap())
            .collect()
            .await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "   def foo():");
    }

    #[tokio::test]
    async fn test_parse_multiline_data() {
        let s = bytes_stream(vec![Bytes::from_static(b"data: line1\ndata: line2\n\n")]);
        let events: Vec<SseEvent> = sse_event_stream(s)
            .map(|r| r.unwrap())
            .collect()
            .await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2");
    }

    #[tokio::test]
    async fn test_eof_partial_frame_is_discarded() {
        let s = bytes_stream(vec![Bytes::from_static(b"data: truncated-without-blank-line")]);
        let events: Vec<SseEvent> = sse_event_stream(s)
            .map(|r| r.unwrap())
            .collect()
            .await;
        assert!(events.is_empty(), "partial EOF frame must not surface: {events:?}");
    }

    #[tokio::test]
    async fn test_oversized_frame_is_shed_and_stream_survives() {
        let big = vec![b'x'; MAX_SSE_FRAME_BYTES + 1024];
        let mut first = b"data: ".to_vec();
        first.extend_from_slice(&big);
        let s = bytes_stream(vec![
            Bytes::from(first),
            Bytes::from_static(b"data: ok\n\n"),
        ]);
        let events: Vec<SseEvent> = sse_event_stream(s)
            .map(|r| r.unwrap())
            .collect()
            .await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "ok");
    }

    #[tokio::test]
    async fn test_anthropic_to_openai_append_done() {
        // Feed a minimal Anthropic message_start + message_stop and ensure
        // output is OpenAI SSE framing terminated by [DONE].
        let start = format!(
            "event: message_start\ndata: {}\n\n",
            serde_json::json!({
                "type": "message_start",
                "message": {"id": "msg_1", "type": "message", "role": "assistant",
                            "content": [], "model": "claude", "stop_reason": null,
                            "stop_sequence": null, "usage": {"input_tokens": 1, "output_tokens": 0}}
            })
        );
        let s = bytes_stream(vec![Bytes::from(start)]);
        let out: Vec<String> = anthropic_sse_to_openai_stream(s, "fallback")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;
        assert!(out.iter().any(|f| f.starts_with("data: {\"id\":\"msg_1\"")), "missing message_start chunk: {out:?}");
        assert!(out.last().unwrap().contains("[DONE]"), "missing [DONE]: {out:?}");
    }

    #[tokio::test]
    async fn test_openai_to_anthropic_emits_anthropic_events() {
        let chunk = serde_json::json!({
            "id": "chatcmpl-1", "object": "chat.completion.chunk", "created": 1,
            "model": "deepseek-v4-flash",
            "choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": null}]
        });
        let s = bytes_stream(vec![Bytes::from(format!("data: {}\n\n", chunk))]);
        let out: Vec<String> = openai_sse_to_anthropic_stream(s, "fallback")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;
        assert!(!out.is_empty(), "no anthropic events emitted");
        assert!(out[0].starts_with("event: "), "not anthropic framing: {out:?}");
        assert!(out[0].contains("message_start"), "missing message_start: {out:?}");
        let has_delta = out.iter().any(|f| f.contains("content_block_delta"));
        assert!(has_delta, "missing content_block_delta: {out:?}");
    }

    #[tokio::test]
    async fn test_openai_to_anthropic_synthesizes_stop_when_upstream_omits_finish() {
        // Upstream never sends finish_reason — only content deltas and [DONE].
        // The translator must synthesize message_delta + message_stop at EOF.
        let chunk = serde_json::json!({
            "id": "chatcmpl-2", "object": "chat.completion.chunk", "created": 1,
            "model": "deepseek-v4-flash",
            "choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": null}]
        });
        let s = bytes_stream(vec![
            Bytes::from(format!("data: {}\n\n", chunk)),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ]);
        let out: Vec<String> = openai_sse_to_anthropic_stream(s, "fallback")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;
        let joined = out.join("");
        assert!(joined.contains("event: message_stop"), "missing synthesized message_stop: {joined}");
        assert!(joined.contains("event: message_delta"), "missing synthesized message_delta: {joined}");
    }

    #[tokio::test]
    async fn test_telemetry_stream_records_flow_and_completion_frame() {
        use ponyllm_core::telemetry::{
            EventBus, EventCtx, MetricsProjection, StreamProjection,
        };
        use crate::frames::FrameConverter;

        let metrics = Arc::new(MetricsCollector::new());
        let recorder = Arc::new(FlightRecorder::new(10));
        let bus = Arc::new(EventBus::new(100));
        bus.add_projection(Arc::new(MetricsProjection::new(metrics.clone())));
        let stream_proj = Arc::new(StreamProjection::default());
        bus.add_projection(stream_proj.clone());
        bus.add_projection(Arc::new(FrameConverter::new(recorder.clone())));
        let start = Instant::now();
        let ctx = StreamFailureContext {
            bus: bus.clone(),
            ctx: EventCtx::new("req-flow-1", "/v1/chat/completions", start),
            provider: "opencode-zen".to_string(),
            stages: Arc::new(Mutex::new(StageTimings::default())),
            request_snippet: None,
            estimated_prompt_tokens: 10,
            attempt_start: Some(start),
        };
        let s = bytes_stream(vec![
            Bytes::from_static(b"data: one\n\n"),
            Bytes::from_static(b"data: two\n\n"),
            Bytes::from_static(b"data: three\n\n"),
        ]);
        let monitored = wrap_telemetry_stream(s, ctx);
        let out: Vec<Bytes> = monitored.map(|r| r.unwrap()).collect().await;
        assert_eq!(out.len(), 3);
        let summary = metrics.get_summary();
        assert_eq!(summary.stream.stream_count, 1);
        assert_eq!(summary.stream.total_chunks, 3);
        assert!(summary.stream.avg_ttft_ms.is_some());
        assert_eq!(stream_proj.node_for("opencode-zen").get_stream_count(), 1);
        // trace stitches the single-append journey by request_id
        let trace = bus.trace_for("req-flow-1");
        assert!(trace.iter().any(|e| matches!(
            e.event,
            GatewayEvent::StreamCompleted { .. }
        )));
        let frames = recorder.get_recent_frames();
        let done = frames.iter().find(|f| {
            f.response_snippet.is_some()
        });
        let done = done.expect("completion frame kept");
        let flow = done.stream_flow.as_ref().expect("flow detail kept");
        assert_eq!(flow.chunks, Some(3));
        assert!(flow.ttft_ms.is_some());
    }

    #[tokio::test]
    async fn test_responses_to_chat_stream_wrapper() {
        let created = format!(
            "event: response.created\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.created",
                "response": {"id": "resp_1", "object": "response", "status": "in_progress", "model": "m", "output": []}
            })
        );
        let delta = format!(
            "event: response.output_text.delta\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.output_text.delta",
                "response_id": "resp_1", "item_id": "it_0",
                "output_index": 0, "content_index": 0, "delta": "hello"
            })
        );
        let done = format!(
            "event: response.completed\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.completed",
                "response": {"id": "resp_1", "object": "response", "status": "completed", "model": "m",
                    "output": [], "usage": {"total_tokens": 9, "input_tokens": 6, "output_tokens": 3}}
            })
        );
        let s = bytes_stream(vec![Bytes::from(created), Bytes::from(delta), Bytes::from(done)]);
        let out: Vec<String> = responses_sse_to_chat_stream(s, "m")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;
        let joined = out.join("");
        assert!(joined.contains("\"content\":\"hello\""), "missing text chunk: {joined}");
        assert!(joined.contains("\"finish_reason\":\"stop\""), "missing finish: {joined}");
        assert!(out.last().unwrap().contains("[DONE]"), "missing [DONE]");
    }

    #[tokio::test]
    async fn test_chat_to_responses_stream_wrapper() {
        let chunk = serde_json::json!({
            "id": "chatcmpl-1", "object": "chat.completion.chunk", "created": 1,
            "model": "m",
            "choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": null}]
        });
        let fin = serde_json::json!({
            "id": "chatcmpl-1", "object": "chat.completion.chunk", "created": 1,
            "model": "m",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
        });
        let s = bytes_stream(vec![
            Bytes::from(format!("data: {}\n\n", chunk)),
            Bytes::from(format!("data: {}\n\n", fin)),
        ]);
        let out: Vec<String> = chat_sse_to_responses_stream(s, "m")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;
        let joined = out.join("");
        assert!(joined.contains("event: response.created"), "missing created: {joined}");
        assert!(joined.contains("event: response.output_text.delta"), "missing delta: {joined}");
        assert!(joined.contains("event: response.completed"), "missing completed: {joined}");
    }

    #[tokio::test]
    async fn test_chat_to_responses_synthesizes_completed_at_eof() {
        let chunk = serde_json::json!({
            "id": "chatcmpl-9", "object": "chat.completion.chunk", "created": 1,
            "model": "m",
            "choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": null}]
        });
        let s = bytes_stream(vec![Bytes::from(format!("data: {}\n\n", chunk))]);
        let out: Vec<String> = chat_sse_to_responses_stream(s, "m")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;
        let joined = out.join("");
        assert!(joined.contains("event: response.completed"), "missing synthesized completed: {joined}");
    }

    #[tokio::test]
    async fn test_responses_to_anthropic_stream_wrapper() {
        let created = format!(
            "event: response.created\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.created",
                "response": {"id": "resp_2", "object": "response", "status": "in_progress", "model": "m", "output": []}
            })
        );
        let delta = format!(
            "event: response.output_text.delta\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.output_text.delta",
                "response_id": "resp_2", "item_id": "it_0",
                "output_index": 0, "content_index": 0, "delta": "yo"
            })
        );
        let done = format!(
            "event: response.completed\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.completed",
                "response": {"id": "resp_2", "object": "response", "status": "completed", "model": "m",
                    "output": [], "usage": {"total_tokens": 5, "input_tokens": 3, "output_tokens": 2}}
            })
        );
        let s = bytes_stream(vec![Bytes::from(created), Bytes::from(delta), Bytes::from(done)]);
        let out: Vec<String> = responses_sse_to_anthropic_stream(s, "m")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;
        let joined = out.join("");
        assert!(joined.contains("event: message_start"), "missing start: {joined}");
        assert!(joined.contains("content_block_delta"), "missing delta: {joined}");
        assert!(joined.contains("event: message_stop"), "missing stop: {joined}");
    }

    #[tokio::test]
    async fn test_anthropic_to_responses_stream_wrapper() {
        let start = format!(
            "event: message_start\ndata: {}\n\n",
            serde_json::json!({
                "type": "message_start",
                "message": {"id": "msg_3", "type": "message", "role": "assistant",
                            "content": [], "model": "m", "stop_reason": null,
                            "stop_sequence": null, "usage": {"input_tokens": 1, "output_tokens": 0}}
            })
        );
        let delta = format!(
            "event: content_block_delta\ndata: {}\n\n",
            serde_json::json!({
                "type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": "hey"}
            })
        );
        let stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let s = bytes_stream(vec![
            Bytes::from(start),
            Bytes::from(delta),
            Bytes::from_static(stop.as_bytes()),
        ]);
        let out: Vec<String> = anthropic_sse_to_responses_stream(s, "m")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;
        let joined = out.join("");
        assert!(joined.contains("event: response.created"), "missing created: {joined}");
        assert!(joined.contains("event: response.output_text.delta"), "missing delta: {joined}");
        assert!(joined.contains("event: response.completed"), "missing completed: {joined}");
    }

    #[tokio::test]
    async fn test_responses_to_chat_synthesizes_finish_at_eof_when_upstream_omits_done() {
        let created = format!(
            "event: response.created\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.created",
                "response": {"id": "resp_eof", "object": "response", "status": "in_progress", "model": "m", "output": []}
            })
        );
        let delta = format!(
            "event: response.output_text.delta\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.output_text.delta",
                "response_id": "resp_eof", "item_id": "it_eof",
                "output_index": 0, "content_index": 0, "delta": "world"
            })
        );
        // Upstream closes stream here without sending response.completed or response.done!
        let s = bytes_stream(vec![Bytes::from(created), Bytes::from(delta)]);
        let out: Vec<String> = responses_sse_to_chat_stream(s, "m")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;
        let joined = out.join("");
        assert!(joined.contains("\"content\":\"world\""), "missing content: {joined}");
        assert!(
            joined.contains("\"finish_reason\":\"stop\""),
            "must synthesize finish_reason:stop at EOF so clients never fail with 'Stream ended without finish_reason': {joined}"
        );
        assert!(out.last().unwrap().contains("[DONE]"), "missing [DONE]: {out:?}");
    }

    #[tokio::test]
    async fn test_responses_to_chat_failed_surfaces_stream_error_not_other() {
        // Upstream `response.failed` must surface as a stream error item
        // carrying the upstream code/message — never as a synthesized
        // `finish_reason:"other"` success chunk.
        let delta = format!(
            "event: response.output_text.delta\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.output_text.delta",
                "response_id": "resp_f", "item_id": "it_f",
                "output_index": 0, "content_index": 0, "delta": "partial"
            })
        );
        let failed = format!(
            "event: response.failed\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.failed",
                "response": {"id": "resp_f", "object": "response", "status": "failed",
                    "model": "m", "output": [],
                    "error": {"code": "server_error", "message": "upstream blew up"}}
            })
        );
        let s = bytes_stream(vec![Bytes::from(delta), Bytes::from(failed)]);
        let out: Vec<Result<Bytes, ResponsesChatStreamError>> =
            responses_sse_to_chat_stream(s, "m").collect().await;
        let ok_text: String = out
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect();
        let err_text: String = out
            .iter()
            .filter_map(|r| r.as_ref().err())
            .map(|e| e.to_string())
            .collect();
        assert!(
            ok_text.contains("\"content\":\"partial\""),
            "partial content before failure must still flow: {ok_text}"
        );
        assert!(
            !ok_text.contains("\"finish_reason\":\"other\""),
            "must never synthesize finish_reason:other for a failed upstream: {ok_text}"
        );
        assert!(
            !ok_text.contains("\"finish_reason\":\"stop\""),
            "must not synthesize a success stop chunk after failure: {ok_text}"
        );
        assert!(
            err_text.contains("server_error") && err_text.contains("upstream blew up"),
            "stream error must retain upstream code/message: {err_text}"
        );
        assert!(
            ok_text.contains("[DONE]"),
            "EOF tail still terminates the (truncated) stream: {ok_text}"
        );
    }

    #[tokio::test]
    async fn test_responses_to_chat_failed_without_error_payload_still_errors() {
        // `response.failed` with no error object falls back to the
        // status/id reason — still a stream error, never `other`.
        let failed = format!(
            "event: response.failed\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.failed",
                "response": {"id": "resp_n", "object": "response", "status": "failed",
                    "model": "m", "output": []}
            })
        );
        let s = bytes_stream(vec![Bytes::from(failed)]);
        let out: Vec<Result<Bytes, ResponsesChatStreamError>> =
            responses_sse_to_chat_stream(s, "m").collect().await;
        assert!(
            out.iter().any(|r| r.is_err()),
            "failed without error payload must still surface a stream error: {out:?}"
        );
        let ok_text: String = out
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect();
        assert!(
            !ok_text.contains("finish_reason"),
            "no terminal finish chunk at all after failure: {ok_text}"
        );
    }

    #[tokio::test]
    async fn test_responses_to_chat_transport_error_still_surfaces() {
        // A mid-stream transport failure keeps flowing as a stream error item
        // (and also suppresses the EOF success synthesis).
        let delta = Bytes::from_static(b"data: {\"type\":\"response.output_text.delta\",\"response_id\":\"r\",\"item_id\":\"i\",\"output_index\":0,\"content_index\":0,\"delta\":\"hi\"}\n\n");
        let s = futures_util::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(delta),
            Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "conn reset")),
        ]);
        let out: Vec<Result<Bytes, ResponsesChatStreamError>> =
            responses_sse_to_chat_stream(s, "m").collect().await;
        let errs: Vec<String> = out
            .iter()
            .filter_map(|r| r.as_ref().err())
            .map(|e| e.to_string())
            .collect();
        assert_eq!(errs.len(), 1, "exactly one stream error expected: {out:?}");
        assert!(
            matches!(
                out.iter().find(|r| r.is_err()),
                Some(Err(ResponsesChatStreamError::Transport(_)))
            ),
            "transport failure must map to Transport: {out:?}"
        );
        assert!(errs[0].contains("conn reset"), "detail retained: {errs:?}");
    }

    #[tokio::test]
    async fn test_responses_to_chat_failed_is_recorded_as_stream_failed() {
        use ponyllm_core::telemetry::{EventBus, EventCtx, MetricsProjection, StreamProjection};
        use crate::frames::FrameConverter;

        let metrics = Arc::new(MetricsCollector::new());
        let recorder = Arc::new(FlightRecorder::new(10));
        let bus = Arc::new(EventBus::new(100));
        bus.add_projection(Arc::new(MetricsProjection::new(metrics.clone())));
        let stream_proj = Arc::new(StreamProjection::default());
        bus.add_projection(stream_proj.clone());
        bus.add_projection(Arc::new(FrameConverter::new(recorder.clone())));
        let start = Instant::now();
        let ctx = StreamFailureContext {
            bus: bus.clone(),
            ctx: EventCtx::new("req-failed-1", "/v1/chat/completions", start),
            provider: "spark-fail".to_string(),
            stages: Arc::new(Mutex::new(StageTimings::default())),
            request_snippet: None,
            estimated_prompt_tokens: 10,
            attempt_start: Some(start),
        };
        let failed = format!(
            "event: response.failed\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.failed",
                "response": {"id": "resp_f", "object": "response", "status": "failed",
                    "model": "m", "output": [],
                    "error": {"code": "server_error", "message": "upstream exploded"}}
            })
        );
        let s = bytes_stream(vec![Bytes::from(failed)]);
        let inner = responses_sse_to_chat_stream(s, "m");
        let monitored = wrap_telemetry_stream(inner, ctx);
        let out: Vec<Result<Bytes, ResponsesChatStreamError>> = monitored.collect().await;
        assert!(
            out.iter().any(|r| r.is_err()),
            "failure must surface as a stream error item: {out:?}"
        );
        let trace = bus.trace_for("req-failed-1");
        assert!(
            trace.iter().any(|e| matches!(
                e.event,
                GatewayEvent::StreamFailed { .. }
            )),
            "telemetry must record StreamFailed (not Completed): {:?}",
            trace.iter().map(|e| format!("{:?}", e.event)).collect::<Vec<_>>()
        );
        assert!(
            !trace.iter().any(|e| matches!(
                e.event,
                GatewayEvent::StreamCompleted { .. }
            )),
            "a failed stream must never record StreamCompleted"
        );
    }

    #[tokio::test]
    async fn test_anthropic_to_openai_synthesizes_finish_at_eof_when_upstream_omits_stop() {
        let start = format!(
            "event: message_start\ndata: {}\n\n",
            serde_json::json!({
                "type": "message_start",
                "message": {"id": "msg_eof", "type": "message", "role": "assistant",
                            "content": [], "model": "m", "stop_reason": null,
                            "stop_sequence": null, "usage": {"input_tokens": 1, "output_tokens": 0}}
            })
        );
        let delta = format!(
            "event: content_block_delta\ndata: {}\n\n",
            serde_json::json!({
                "type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": "streaming content"}
            })
        );
        // Upstream abruptly ends without message_delta(stop_reason) or message_stop!
        let s = bytes_stream(vec![Bytes::from(start), Bytes::from(delta)]);
        let out: Vec<String> = anthropic_sse_to_openai_stream(s, "m")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;
        let joined = out.join("");
        assert!(joined.contains("\"content\":\"streaming content\""), "missing content: {joined}");
        assert!(
            joined.contains("\"finish_reason\":\"stop\""),
            "must synthesize finish_reason:stop at EOF for anthropic->openai: {joined}"
        );
        assert!(out.last().unwrap().contains("[DONE]"), "missing [DONE]: {out:?}");
    }

    #[tokio::test]
    async fn test_collect_antigravity_sse_to_json() {
        let chunk1 = format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{"text": "Hello, "}]
                        }
                    }]
                }
            })
        );
        let chunk2 = format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{"text": "world!"}]
                        },
                        "finishReason": "STOP"
                    }],
                    "usageMetadata": {
                        "promptTokenCount": 5,
                        "candidatesTokenCount": 2,
                        "totalTokenCount": 7
                    }
                }
            })
        );
        let done = "data: [DONE]\n\n";

        let s = bytes_stream(vec![
            Bytes::from(chunk1),
            Bytes::from(chunk2),
            Bytes::from_static(done.as_bytes()),
        ]);

        let json_val = collect_antigravity_sse_to_json(s).await.expect("collect should succeed");
        assert_eq!(
            json_val["candidates"][0]["content"]["parts"][0]["text"],
            "Hello, world!"
        );
        assert_eq!(json_val["candidates"][0]["finishReason"], "STOP");
        assert_eq!(json_val["usageMetadata"]["totalTokenCount"], 7);
    }

    #[tokio::test]
    async fn test_collect_sse_with_error_returns_err() {
        let err_chunk = format!(
            "data: {}\n\n",
            serde_json::json!({
                "error": {
                    "code": 503,
                    "message": "The model is overloaded."
                }
            })
        );
        let s = bytes_stream(vec![Bytes::from(err_chunk)]);
        let res = collect_antigravity_sse_to_json(s).await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err();
        assert!(
            err_msg.contains("Antigravity stream error frame: The model is overloaded."),
            "actual: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_collect_sse_with_response_error_returns_err() {
        let err_chunk = format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "error": {
                        "code": 429,
                        "message": "Resource has been exhausted"
                    }
                }
            })
        );
        let s = bytes_stream(vec![Bytes::from(err_chunk)]);
        let res = collect_antigravity_sse_to_json(s).await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err();
        assert!(
            err_msg.contains("Antigravity stream error frame: Resource has been exhausted"),
            "actual: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_sse_chunk_timeout_guard() {
        let stream = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
        let res = collect_antigravity_sse_to_json_with_timeout(stream, std::time::Duration::from_millis(30)).await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err();
        assert!(
            err_msg.contains("Antigravity SSE stream stalled:"),
            "actual: {}",
            err_msg
        );
        assert!(
            err_msg.contains("chunk timeout exceeded"),
            "actual: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_collect_antigravity_sse_only_thought_zero_text_content() {
        let chunk1 = format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{"thought": true, "text": "Thinking process only..."}]
                        },
                        "finishReason": "MAX_TOKENS"
                    }],
                    "usageMetadata": {
                        "promptTokenCount": 10,
                        "candidatesTokenCount": 2048,
                        "totalTokenCount": 2058
                    }
                }
            })
        );
        let done = "data: [DONE]\n\n";

        let s = bytes_stream(vec![
            Bytes::from(chunk1),
            Bytes::from_static(done.as_bytes()),
        ]);

        let json_val = collect_antigravity_sse_to_json(s).await.expect("collect should succeed");
        assert_eq!(
            json_val["candidates"][0]["content"]["parts"][0]["text"],
            "Thinking process only..."
        );
        assert_eq!(
            json_val["candidates"][0]["content"]["parts"][0]["thought"],
            true
        );
        assert_eq!(json_val["candidates"][0]["finishReason"], "MAX_TOKENS");
        assert_eq!(json_val["usageMetadata"]["candidatesTokenCount"], 2048);
    }

    #[tokio::test]
    async fn test_collect_antigravity_sse_zero_content_stop_returns_err() {
        // When upstream sends a STOP finishReason with zero text and zero tool calls,
        // collect_antigravity_sse_to_json returns an Err to trigger transparent gateway-side retry.
        let chunk1 = format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": []
                        },
                        "finishReason": "STOP"
                    }]
                }
            })
        );
        let s = bytes_stream(vec![
            Bytes::from(chunk1),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ]);

        let res = collect_antigravity_sse_to_json(s).await;
        assert!(res.is_err(), "Must return Err on transient empty STOP to trigger gateway retry");
        let err_msg = res.unwrap_err();
        assert!(err_msg.contains("transient empty STOP"), "Error message must indicate transient empty STOP: {}", err_msg);
    }

    #[tokio::test]
    async fn test_antigravity_sse_to_openai_stream_zero_content_emits_error_frame() {
        // When upstream sends a candidate that has only empty parts or no content
        // followed by finishReason STOP, antigravity_sse_to_openai_stream must NOT emit
        // a deceptive empty stop chunk followed by [DONE]; it must emit an SSE error frame
        // to clearly notify downstream clients of the upstream failure.
        let empty_chunk = format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": []
                        },
                        "finishReason": "STOP"
                    }]
                }
            })
        );
        let s = bytes_stream(vec![
            Bytes::from(empty_chunk),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ]);

        let out: Vec<String> = antigravity_sse_to_openai_stream(s, "gemini-3.8-flash-high")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;

        let joined = out.join("");
        assert!(!joined.contains("\"finish_reason\":\"stop\""), "Stream must NOT emit deceptive stop on zero content: {}", joined);
        assert!(joined.contains("\"code\":\"EMPTY_RESPONSE\""), "Stream must emit EMPTY_RESPONSE error frame: {}", joined);
        assert!(joined.ends_with("data: [DONE]\n\n"), "Stream must end with [DONE]");
    }

    #[tokio::test]
    async fn test_antigravity_sse_to_anthropic_stream_zero_content_emits_error_frame() {
        let empty_chunk = format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": []
                        },
                        "finishReason": "STOP"
                    }]
                }
            })
        );
        let s = bytes_stream(vec![
            Bytes::from(empty_chunk),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ]);

        let out: Vec<String> = antigravity_sse_to_anthropic_stream(s, "gemini-3.8-flash-high")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;

        let joined = out.join("");
        assert!(joined.contains("api_error"), "Anthropic stream must emit error event: {}", joined);
        assert!(joined.contains("returned a completed response with no content"), "Error message must indicate empty STOP: {}", joined);
    }

    #[tokio::test]
    async fn test_antigravity_sse_to_openai_stream_upstream_error_frame_does_not_panic() {
        let err_chunk = format!(
            "data: {}\n\n",
            serde_json::json!({
                "error": {
                    "code": 429,
                    "message": "Resource has been exhausted (e.g. check quota)."
                }
            })
        );
        let s = bytes_stream(vec![
            Bytes::from(err_chunk),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ]);

        let out: Vec<String> = antigravity_sse_to_openai_stream(s, "gemini-3.8-flash-high")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;

        let joined = out.join("");
        assert_eq!(joined, "data: [DONE]\n\n", "Error frame must be logged without panicking or creating fake choices");
    }

    #[tokio::test]
    async fn test_antigravity_sse_to_anthropic_stream_upstream_error_frame_does_not_panic() {
        let err_chunk = format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "error": {
                        "code": 500,
                        "message": "Internal error encountered."
                    }
                }
            })
        );
        let s = bytes_stream(vec![
            Bytes::from(err_chunk),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ]);

        let out: Vec<String> = antigravity_sse_to_anthropic_stream(s, "claude-sonnet-4-6")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;

        // Clean finish without panic
        assert!(out.is_empty() || out.iter().all(|s| s.is_empty() || s.starts_with("event:")));
    }

    #[tokio::test]
    async fn test_antigravity_sse_to_openai_stream_multiframe_tool_calls_stickiness() {
        // Frame 1: emits functionCall without finishReason
        let frame1 = format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{
                                "functionCall": {
                                    "name": "bash",
                                    "args": {
                                        "command": "ls"
                                    }
                                }
                            }]
                        }
                    }]
                }
            })
        );
        // Frame 2: terminal frame with empty parts and finishReason STOP
        let frame2 = format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": []
                        },
                        "finishReason": "STOP"
                    }]
                }
            })
        );
        let s = bytes_stream(vec![
            Bytes::from(frame1),
            Bytes::from(frame2),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ]);

        let out: Vec<String> = antigravity_sse_to_openai_stream(s, "gemini-3.8-flash-high")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;

        let joined = out.join("");
        // Must override Stop to tool_calls!
        assert!(joined.contains("\"finish_reason\":\"tool_calls\""), "Stream must stickily preserve tool_calls finish reason: {}", joined);
        assert!(!joined.contains("\"finish_reason\":\"stop\""), "Stream must NOT regress to stop when tool_calls were emitted: {}", joined);
        assert!(joined.ends_with("data: [DONE]\n\n"), "Stream must end with [DONE]");
    }

    #[tokio::test]
    async fn test_antigravity_sse_to_openai_stream_abrupt_empty_eof_does_not_synthesize_stop() {
        // When upstream terminates abruptly without emitting ANY chunk (e.g. immediate EOF),
        // antigravity_sse_to_openai_stream must NOT synthesize a fake Stop chunk.
        // It must cleanly end with [DONE] so downstream knows zero output was generated.
        let s = bytes_stream(vec![]);

        let out: Vec<String> = antigravity_sse_to_openai_stream(s, "gemini-3.8-flash-high")
            .map(|r| String::from_utf8_lossy(&r.unwrap()).to_string())
            .collect()
            .await;

        let joined = out.join("");
        assert!(!joined.contains("\"finish_reason\":\"stop\""), "Must not synthesize fake stop chunk on empty stream: {}", joined);
        assert_eq!(joined, "data: [DONE]\n\n", "Must only contain [DONE]");
    }

    #[tokio::test]
    async fn test_telemetry_stream_accurate_tps_calculation() {
        use ponyllm_core::telemetry::{EventBus, EventCtx, MetricsProjection, StreamProjection};
        let metrics = Arc::new(MetricsCollector::new());
        let bus = Arc::new(EventBus::new(100));
        bus.add_projection(Arc::new(MetricsProjection::new(metrics.clone())));
        let stream_proj = Arc::new(StreamProjection::default());
        bus.add_projection(stream_proj.clone());

        let start = Instant::now();
        let ctx = StreamFailureContext {
            bus: bus.clone(),
            ctx: EventCtx::new("req-tps-1", "/v1/chat/completions", start),
            provider: "test-provider".to_string(),
            stages: Arc::new(Mutex::new(StageTimings::default())),
            request_snippet: None,
            estimated_prompt_tokens: 10,
            attempt_start: Some(start),
        };

        // Two chunks containing real content: total 20 characters (~6-7 tokens)
        // With previous flawed bytes/3 heuristic, 400+ bytes wire SSE would produce ~140 tokens and inflated TPS > 2000.
        let chunk1 = Bytes::from_static(b"data: {\"id\":\"1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello world \"}}]}\n\n");
        let chunk2 = Bytes::from_static(b"data: {\"id\":\"1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"test message\"}}]}\n\n");
        let s = bytes_stream(vec![chunk1, chunk2]);

        let monitored = wrap_telemetry_stream(s, ctx);
        let _out: Vec<Bytes> = monitored.map(|r| r.unwrap()).collect().await;

        let node = stream_proj.node_for("test-provider");
        let snap = node.flow_snapshot();
        // Accurate token estimate: 24 chars / 3 = 8 tokens
        // Check that tps is within realistic LLM range, not thousands
        assert!(snap.tps < 300.0, "TPS should be reasonably bounded, got {}", snap.tps);
        assert_eq!(snap.stream_count, 1);
    }

    #[tokio::test]
    async fn test_verify_antigravity_stream_preamble_content_ready() {
        let comment = Bytes::from_static(b": keepalive\n\n");
        let content_chunk = Bytes::from(format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{"text": "Hello world!"}]
                        }
                    }]
                }
            })
        ));
        let s = bytes_stream(vec![comment, content_chunk]);

        let res = verify_antigravity_stream_preamble(s, std::time::Duration::from_secs(1))
            .await
            .expect("verification should succeed");

        match res {
            AntigravityPreambleResult::Ready { buffered, .. } => {
                assert_eq!(buffered.len(), 2);
            }
            other => panic!("Expected Ready, got {:?}", match other {
                AntigravityPreambleResult::TransientEmptyStop { .. } => "TransientEmptyStop",
                AntigravityPreambleResult::DeterministicBlock { .. } => "DeterministicBlock",
                AntigravityPreambleResult::AbruptTermination => "AbruptTermination",
                _ => "Other",
            }),
        }
    }

    #[tokio::test]
    async fn test_verify_antigravity_stream_preamble_thought_ready() {
        let thought_chunk = Bytes::from(format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{"thought": true, "text": "Thinking..."}]
                        }
                    }]
                }
            })
        ));
        let s = bytes_stream(vec![thought_chunk]);

        let res = verify_antigravity_stream_preamble(s, std::time::Duration::from_secs(1))
            .await
            .expect("verification should succeed");

        assert!(matches!(res, AntigravityPreambleResult::Ready { .. }));
    }

    #[tokio::test]
    async fn test_verify_antigravity_stream_preamble_tool_call_ready() {
        let tool_chunk = Bytes::from(format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{
                                "functionCall": {
                                    "name": "bash",
                                    "args": {"command": "ls"}
                                }
                            }]
                        }
                    }]
                }
            })
        ));
        let s = bytes_stream(vec![tool_chunk]);

        let res = verify_antigravity_stream_preamble(s, std::time::Duration::from_secs(1))
            .await
            .expect("verification should succeed");

        assert!(matches!(res, AntigravityPreambleResult::Ready { .. }));
    }

    #[tokio::test]
    async fn test_verify_antigravity_stream_preamble_empty_stop() {
        let ping = Bytes::from_static(b": ping\n\n");
        let empty_stop = Bytes::from(format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": []
                        },
                        "finishReason": "STOP"
                    }]
                }
            })
        ));
        let s = bytes_stream(vec![ping, empty_stop]);

        let res = verify_antigravity_stream_preamble(s, std::time::Duration::from_secs(1))
            .await
            .expect("verification should succeed");

        assert!(matches!(res, AntigravityPreambleResult::TransientEmptyStop { .. }));
    }

    #[tokio::test]
    async fn test_verify_antigravity_stream_preamble_safety_block() {
        let safety_chunk = Bytes::from(format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "promptFeedback": {
                        "blockReason": "SAFETY"
                    }
                }
            })
        ));
        let s = bytes_stream(vec![safety_chunk]);

        let res = verify_antigravity_stream_preamble(s, std::time::Duration::from_secs(1))
            .await
            .expect("verification should succeed");

        match res {
            AntigravityPreambleResult::DeterministicBlock { reason } => {
                assert!(reason.contains("safety block"));
            }
            _ => panic!("Expected DeterministicBlock"),
        }
    }

    #[tokio::test]
    async fn test_verify_preamble_keepalive_burst_does_not_commit_zero_content() {
        // Regression: a warm-up burst of keepalive comments must not consume the
        // preamble frame budget. 10 pings + late empty STOP must be classified as
        // TransientEmptyStop (retryable), never Ready-then-leak to the client.
        let mut chunks: Vec<Bytes> = (0..10)
            .map(|i| Bytes::from(format!(": keepalive {}\n\n", i)))
            .collect();
        chunks.push(Bytes::from(format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {"role": "model", "parts": []},
                        "finishReason": "STOP"
                    }]
                }
            })
        )));
        let s = bytes_stream(chunks);

        let res = verify_antigravity_stream_preamble(s, std::time::Duration::from_secs(1))
            .await
            .expect("verification should succeed");

        assert!(
            matches!(res, AntigravityPreambleResult::TransientEmptyStop { .. }),
            "keepalive burst followed by empty STOP must stay retryable"
        );
    }

    #[tokio::test]
    async fn test_verify_preamble_role_only_frames_do_not_consume_budget() {
        // Role-only candidate frames (no parts, no finishReason) are not
        // significant: several of them followed by an empty STOP must remain
        // retryable instead of tripping the max_frames Ready valve.
        let role_frame = Bytes::from(format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {"role": "model", "parts": []}
                    }]
                }
            })
        ));
        let empty_stop = Bytes::from(format!(
            "data: {}\n\n",
            serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {"role": "model", "parts": []},
                        "finishReason": "STOP"
                    }]
                }
            })
        ));
        let s = bytes_stream(vec![role_frame; 10].into_iter().chain(std::iter::once(empty_stop)).collect());

        let res = verify_antigravity_stream_preamble(s, std::time::Duration::from_secs(1))
            .await
            .expect("verification should succeed");

        assert!(matches!(res, AntigravityPreambleResult::TransientEmptyStop { .. }));
    }

    #[tokio::test]
    async fn test_verify_preamble_overall_deadline_returns_ready() {
        // A stream that never yields must hit the overall preamble deadline and
        // return Ready (best-effort commit), not stall the request forever.
        let s = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
        let res = verify_antigravity_stream_preamble_with_deadline(
            s,
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(80),
        )
        .await
        .expect("verification should succeed");

        assert!(matches!(res, AntigravityPreambleResult::Ready { .. }));
    }

    #[test]
    fn test_empty_stop_retry_delay_bounds() {
        // 1-based attempt schedule: 250ms doubling to a 2s cap, jitter ±25%.
        let first = empty_stop_retry_delay(1);
        assert!(
            first >= std::time::Duration::from_millis(187) && first <= std::time::Duration::from_millis(313),
            "first retry delay out of jitter band: {:?}",
            first
        );
        for attempt in 4..=8 {
            let d = empty_stop_retry_delay(attempt);
            assert!(
                d >= std::time::Duration::from_millis(1500) && d <= std::time::Duration::from_millis(2500),
                "capped delay out of jitter band at attempt {}: {:?}",
                attempt,
                d
            );
        }
    }

    #[test]
    fn test_is_transient_empty_stop_error_classification() {
        assert!(is_transient_empty_stop_error(
            "Antigravity stream completed with zero text and zero tool calls (transient empty STOP)"
        ));
        assert!(!is_transient_empty_stop_error(
            "Antigravity stream error frame: quota exceeded"
        ));
        assert!(!is_transient_empty_stop_error(
            "Antigravity SSE stream stalled: 15s chunk timeout exceeded"
        ));
    }
}


