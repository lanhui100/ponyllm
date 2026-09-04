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

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use std::sync::Arc;
use std::time::Instant;
use parking_lot::Mutex;
use ponyllm_core::telemetry::{
    gap_percentiles, EventBus, EventCtx, GatewayEvent, StageTimings, StreamFlowSample,
};
use ponyllm_protocol::anthropic::messages::MessageStreamEvent;
use ponyllm_protocol::openai::chat::ChatCompletionChunk;
use ponyllm_protocol::translator::{
    AnthropicStreamToChatFsm, ChatStreamToAnthropicFsm,
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

/// Extract the first complete SSE event (bounded by a blank line) from `buf`.
/// Returns `(event, remaining_bytes)` when a full frame is present.
fn extract_event(buf: &[u8]) -> Option<(SseEvent, Vec<u8>)> {
    let mut i = 0;
    while i + 1 < buf.len() {
        // \n\n boundary
        if buf[i] == b'\n' && buf[i + 1] == b'\n' {
            let (head, tail) = buf.split_at(i + 2);
            return Some((parse_event_lines(head), tail.to_vec()));
        }
        // \r\n\r\n boundary
        if i + 3 < buf.len()
            && buf[i..i + 4] == [b'\r', b'\n', b'\r', b'\n']
        {
            let (head, tail) = buf.split_at(i + 4);
            return Some((parse_event_lines(head), tail.to_vec()));
        }
        i += 1;
    }
    None
}

/// Parse the head block of one SSE frame into an `SseEvent`.
fn parse_event_lines(block: &[u8]) -> SseEvent {
    let text = String::from_utf8_lossy(block);
    let mut event = "message".to_string();
    let mut data_lines: Vec<String> = Vec::new();
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
            data_lines.push(val.to_string());
        }
        // id / retry / other fields are ignored (not needed by the translators)
    }
    SseEvent {
        event,
        data: data_lines.join("\n"),
    }
}

/// Convert a byte stream into a stream of parsed SSE frames, reassembling
/// frames that are split across network chunks.
pub fn sse_event_stream<S, E>(
    stream: S,
) -> impl Stream<Item = Result<SseEvent, E>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let buffer: Vec<u8> = Vec::new();
    let inner = Box::pin(stream);
    futures_util::stream::unfold((inner, buffer), |(mut st, mut buf)| async move {
        loop {
            if let Some((evt, rest)) = extract_event(&buf) {
                buf = rest;
                return Some((Ok(evt), (st, buf)));
            }
            match st.next().await {
                Some(Ok(bytes)) => buf.extend_from_slice(&bytes),
                Some(Err(e)) => return Some((Err(e), (st, buf))),
                None => {
                    // EOF: flush any remaining partial frame as a final event
                    if !buf.is_empty() {
                        let evt = parse_event_lines(&buf);
                        buf.clear();
                        return Some((Ok(evt), (st, buf)));
                    }
                    return None;
                }
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

/// Translate an upstream **OpenAI** SSE byte stream into **Anthropic** SSE
/// frames. Used by `/v1/messages` when the routed upstream is OpenAI-compatible.
pub fn openai_sse_to_anthropic_stream<S, E>(
    stream: S,
    fallback_model: &str,
) -> impl Stream<Item = Result<Bytes, std::convert::Infallible>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let mut fsm = ChatStreamToAnthropicFsm::new(fallback_model);
    // Track whether the FSM already emitted message_stop; if the upstream never
    // sends a finish_reason chunk, we synthesize the terminal events at EOF so
    // Anthropic clients never hang waiting for the message to conclude.
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped_flag = stopped.clone();

    let translated = sse_event_stream(stream).flat_map(move |res| {
        let mut out: Vec<Bytes> = Vec::new();
        match res {
            Ok(evt) => {
                let data = evt.data.trim();
                if data.is_empty() || data == "[DONE]" {
                    // terminal / heartbeat frame: nothing to forward
                } else if let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) {
                    if let Ok(events) = fsm.process_chunk(chunk) {
                        for e in events {
                            if matches!(e, MessageStreamEvent::MessageStop) {
                                stopped_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            }
                            if let Some(b) = anthropic_event_to_sse_bytes(&e) {
                                out.push(b);
                            }
                        }
                    }
                }
            }
            Err(_) => {
                out.push(Bytes::from(
                    "event: error\ndata: {\"type\":\"api_error\",\"message\":\"stream read error\"}\n\n",
                ));
            }
        }
        let iter = futures_util::stream::iter(out.into_iter().map(Ok::<_, std::convert::Infallible>));
        futures_util::stream::BoxStream::from(Box::pin(iter)
            as std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, std::convert::Infallible>> + Send>>)
    });

    // At stream end, guarantee the Anthropic conversation terminates.
    translated
        .chain(futures_util::stream::once(async move {
            let synthetic = if !stopped.load(std::sync::atomic::Ordering::SeqCst) {
                Bytes::from(
                    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                )
            } else {
                Bytes::new()
            };
            Ok::<_, std::convert::Infallible>(synthetic)
        }))
        .boxed()
}

/// Translate an upstream **Anthropic** SSE byte stream into **OpenAI** SSE
/// frames (`data: {chunk}\n\n`, terminating with `data: [DONE]`). Used by
/// `/v1/chat/completions` when the routed upstream is Anthropic-compatible.
pub fn anthropic_sse_to_openai_stream<S, E>(
    stream: S,
    fallback_model: &str,
) -> impl Stream<Item = Result<Bytes, std::convert::Infallible>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let mut fsm = AnthropicStreamToChatFsm::new(fallback_model);
    let translated = sse_event_stream(stream).flat_map(move |res| {
        let mut out: Vec<Bytes> = Vec::new();
        match res {
            Ok(evt) => {
                if let Ok(msge) = serde_json::from_str::<MessageStreamEvent>(&evt.data) {
                    if let Ok(chunks) = fsm.process_event(msge) {
                        for c in chunks {
                            if let Ok(json) = serde_json::to_string(&c) {
                                out.push(Bytes::from(format!("data: {}\n\n", json)));
                            }
                        }
                    }
                }
            }
            Err(_) => {
                out.push(Bytes::from(
                    "data: {\"error\":{\"message\":\"stream read error\",\"type\":\"upstream_error\"}}\n\n",
                ));
            }
        }
        let iter = futures_util::stream::iter(out.into_iter().map(Ok::<_, std::convert::Infallible>));
        futures_util::stream::BoxStream::from(Box::pin(iter)
            as std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, std::convert::Infallible>> + Send>>)
    });

    // OpenAI streams must terminate with `data: [DONE]`.
    translated
        .chain(futures_util::stream::once(async {
            Ok::<_, std::convert::Infallible>(Bytes::from("data: [DONE]\n\n"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use ponyllm_core::telemetry::{FlightRecorder, GatewayEvent, MetricsCollector};

    fn bytes_stream(chunks: Vec<Bytes>) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
        futures_util::stream::iter(chunks.into_iter().map(Ok::<Bytes, std::io::Error>))
    }

    #[tokio::test]
    async fn test_extract_event_split_across_chunks() {
        let s = bytes_stream(vec![
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
}
