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
pub fn responses_sse_to_chat_stream<S, E>(
    stream: S,
    fallback_model: &str,
) -> impl Stream<Item = Result<Bytes, E>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let mut fsm = ResponsesToChatFsm::new(fallback_model);
    let translated = sse_event_stream(stream).flat_map(move |res| {
        let mut out: Vec<Result<Bytes, E>> = Vec::new();
        match res {
            Ok(evt) => {
                if let Ok(msge) = serde_json::from_str::<ResponseStreamEvent>(&evt.data) {
                    if let Ok(chunks) = fsm.process_event(msge) {
                        for c in chunks {
                            if let Ok(json) = serde_json::to_string(&c) {
                                out.push(Ok(Bytes::from(format!("data: {}\n\n", json))));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                out.push(Err(e));
            }
        }
        let iter = futures_util::stream::iter(out);
        futures_util::stream::BoxStream::from(Box::pin(iter)
            as std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, E>> + Send>>)
    });

    translated
        .chain(futures_util::stream::once(async {
            Ok::<_, E>(Bytes::from("data: [DONE]\n\n"))
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
                            if matches!(e, ResponseStreamEvent::Completed { .. }) {
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
                            if matches!(e, ResponseStreamEvent::Completed { .. }) {
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
    let mut fsm = AnthropicStreamToChatFsm::new(fallback_model);
    let translated = sse_event_stream(stream).flat_map(move |res| {
        let mut out: Vec<Result<Bytes, E>> = Vec::new();
        match res {
            Ok(evt) => {
                if let Ok(msge) = serde_json::from_str::<MessageStreamEvent>(&evt.data) {
                    if let Ok(chunks) = fsm.process_event(msge) {
                        for c in chunks {
                            if let Ok(json) = serde_json::to_string(&c) {
                                out.push(Ok(Bytes::from(format!("data: {}\n\n", json))));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                out.push(Err(e));
            }
        }
        let iter = futures_util::stream::iter(out);
        futures_util::stream::BoxStream::from(Box::pin(iter)
            as std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, E>> + Send>>)
    });

    // OpenAI streams must terminate with `data: [DONE]`.
    translated
        .chain(futures_util::stream::once(async {
            Ok::<_, E>(Bytes::from("data: [DONE]\n\n"))
        }))
        .boxed()
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
            has_error: false,
            completed: false,
            last_error: None,
        }
    }

    fn observe_chunk(&mut self, bytes: usize) {
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
        self.bytes_emitted += bytes as u64;
    }

    fn build_flow(&self, now: Instant) -> (StreamFlowSample, Option<f64>) {
        let start = self.failure_ctx.ctx.start;
        let ttft_ms = self.first_token_time.map(|t| {
            (t.saturating_duration_since(start).as_secs_f64() * 1000.0).max(1.0)
        });
        let ttlb_ms = now.saturating_duration_since(start).as_secs_f64() * 1000.0;
        let tps = if let Some(ft) = self.first_token_time {
            let gen_dur = now.saturating_duration_since(ft).as_secs_f64();
            if gen_dur > 0.05 && self.chunks_emitted > 0 {
                Some((self.chunks_emitted as f64 / gen_dur).max(1.0))
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
            ttft_ms,
            ttlb_ms,
            chunks: self.chunks_emitted,
            bytes: self.bytes_emitted,
            max_gap_ms: max_gap,
            stall_count: self.stall_count,
            tps,
            tpot_p50_ms: p50,
            tpot_p95_ms: p95,
            tpot_mean_ms: avg_gap,
        };
        (sample, avg_gap)
    }

    fn emit(&self, provider: Option<String>, event: GatewayEvent) {
        let fctx = &self.failure_ctx;
        fctx.bus.append(&fctx.ctx, provider.or(Some(fctx.provider.clone())), event);
    }

    fn finish_stages(&self, ttft_ms: Option<f64>) -> StageTimings {
        let mut stages = self.failure_ctx.stages.lock().clone();
        stages.downstream_ttft_ms = ttft_ms;
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
                let n = item.len();
                self.observe_chunk(n);
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
                    let stages = self.finish_stages(sample.ttft_ms);
                    if self.has_error {
                        self.emit_failure("stream terminated with error", Some(sample), stages);
                    } else {
                        self.emit(
                            None,
                            GatewayEvent::StreamCompleted {
                                flow: sample,
                                stages,
                                request_snippet: self
                                    .failure_ctx
                                    .request_snippet
                                    .clone(),
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
                let stages = self.finish_stages(sample.ttft_ms);
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

/// Extract prompt_tokens and completion_tokens from OpenAI/Anthropic JSON usage object
pub fn extract_usage_tokens(val: &serde_json::Value) -> (u64, u64) {
    if let Some(usage) = val.get("usage") {
        let prompt = if let Some(p) = usage.get("prompt_tokens").and_then(parse_lenient_u64) {
            p
        } else {
            // Anthropic 兼容处理：总 Prompt = input + cache_read + cache_creation
            let input = usage.get("input_tokens").and_then(parse_lenient_u64).unwrap_or(0);
            let cached_read = usage.get("cache_read_input_tokens").and_then(parse_lenient_u64).unwrap_or(0);
            let cached_create = usage.get("cache_creation_input_tokens").and_then(parse_lenient_u64).unwrap_or(0);
            input.saturating_add(cached_read).saturating_add(cached_create)
        };

        let completion = usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(parse_lenient_u64)
            .unwrap_or(0);

        (prompt, completion)
    } else {
        (0, 0)
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
            f.response_snippet.as_deref().is_some_and(|s| s.contains("[STREAM_COMPLETED"))
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
}
