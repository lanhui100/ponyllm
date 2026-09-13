# Agent Note: Antigravity streaming preamble validation and transparent gateway retry

Status: implemented

## Problem

When AI coding agents (such as DeepSeek Harness via `@earendil-works/pi-ai`, Claude Code, and Codex) send streaming chat completion requests (`stream: true`) to Google Antigravity models (`gemini-3.8-flash-high`), Google's upstream servers intermittently encounter transient overload or prompt prefill blips. Under these conditions, the upstream emits an immediate completion terminating with `finishReason: "STOP"` while generating zero content tokens (0 text bytes, 0 reasoning/thought bytes, 0 tool calls).

In prior versions:
1. In non-streaming requests, `collect_antigravity_sse_to_json` intercepted empty `STOP` completions and returned an error, triggering `UpstreamExecutor`'s transparent key rotation and retry loop without surfacing any error downstream.
2. In streaming requests, however, `ponyllm-server` committed downstream HTTP 200 `text/event-stream` response headers as soon as the upstream connection handshake succeeded. Once the HTTP headers were flushed, the gateway could no longer fail over to another key.
3. Although `antigravity_sse_to_openai_stream` suppressed the deceptive empty STOP chunk and emitted an OpenAI-compatible SSE error event (`EMPTY_RESPONSE`), downstream agent runtimes (specifically DSH `pi-ai`) aborted the agent turn with:
   `model "gemini-3.8-flash-high" returned a completed response with no content (upstream transient empty STOP)`.

An adversarial architectural audit revealed that a naive "first-chunk peek" in route handlers has multiple critical flaws:
- Upstream `reqwest` chunks are arbitrary TCP byte buffers (`Bytes`), which can slice SSE frames midway through JSON payloads.
- Upstream preambles often contain keepalive comments (`: ping`), empty metadata frames, or empty thought blocks before emitting the empty STOP frame.
- Safety / content moderation blocks (`finishReason: "SAFETY"`, `promptFeedback.blockReason`) also emit zero content but are deterministic; retrying them across all pooled keys exhausts quotas and risks mass account suspension.
- Committing HTTP 200 before semantic validation marks defective attempts as successful in the key pool metrics.

## Decision

We implement an upstream streaming head-preamble verifier and transparent retry mechanism within `UpstreamExecutor`:

1. **Preamble Buffering & Semantic Validation (`verify_antigravity_stream_preamble`)**:
   - Before committing HTTP 200 headers to the downstream client, `UpstreamExecutor` accumulates initial SSE frames through an incremental frame boundary parser.
   - **Content Detection**: As soon as any valid content is detected (non-empty `text`, non-empty `thought`, or `functionCall`), validation succeeds immediately. The buffered raw `Bytes` and the remaining upstream stream are re-assembled using zero-copy `stream::iter(buffered).chain(tail_stream)` and returned for downstream streaming. Perceived TTFT is preserved with zero latency penalty.
   - **Transient Empty STOP Detection**: If the upstream stream reaches a candidate with `finishReason == "STOP"` (or `[DONE]` / stream EOF) while cumulative content bytes remain zero, the attempt is classified as a transient upstream defect (`GatewayErrorKind::UpstreamUnavailable`).
   - **Deterministic Safety Short-Circuit**: If `promptFeedback.blockReason` or `finishReason == "SAFETY"` is detected, the attempt is immediately rejected as a client error (HTTP 400 Bad Request) without rotating keys.
   - **Bounded Circuit Breakers**: Upstream preambles are bounded by a maximum of 8 frames, 64 KB total buffered bytes, and a 10-second poll timeout. If limits are exceeded without a conclusive verdict, the buffer is flushed downstream to avoid blocking legitimate non-standard responses.

2. **Transparent Key Rotation on Transient Empty STOP in Streaming Requests**:
   - In `UpstreamExecutor::execute_stream_request_with_timing`, when `verify_antigravity_stream_preamble` returns a transient empty STOP error:
     - The current key is NOT recorded as a successful request in the key pool.
     - The defective key is recorded with a transient network error, and the loop rotates to the next available key candidate.
     - Downstream clients never observe the empty STOP frame or the in-band `EMPTY_RESPONSE` abort; they receive the successfully regenerated stream from the fallback key transparently.

3. **Downstream Safety Net Preservation**:
   - `antigravity_sse_to_openai_stream` and `antigravity_sse_to_anthropic_stream` retain their in-band empty STOP suppression and error framing as a defense-in-depth safety net in case all retries are exhausted.

## Alternatives considered

- **Naive single-chunk peek in route handlers**: Rejected. Raw `Bytes` chunks do not align with SSE frame boundaries; comment frames (`: ping`) and multi-frame preambles cause false classifications and JSON parse failures.
- **Rely solely on downstream client retries**: Rejected. Downstream agent loops in DSH, Claude Code, and Cursor crash on terminal empty STOP completions; handling this at the gateway layer provides transparent self-healing.
- **Full stream buffering**: Rejected. Buffering the entire response destroys streaming interactivity and degrades TTFT. Buffering only the preamble up to the first content token adds zero perceived delay to user output.
- **Synthesizing dummy tokens (whitespace)**: Rejected. Corrupts structured outputs, confuses tool calling parsers, and masks upstream degradation.

## Consequences

- Transient empty `STOP` completions from `gemini-3.8-flash-high` are transparently retried with alternate keys before downstream HTTP headers are committed.
- Zero-content aborts in DeepSeek Harness (`upstream transient empty STOP`) are eliminated.
- Normal TTFT is preserved; valid content releases the stream immediately upon the first content/thought chunk.
- Safety rejections fail fast without burning pooled keys.
- Comprehensive unit and integration tests cover keepalives, thought preambles, tool calls, empty STOP, and safety blocks.
