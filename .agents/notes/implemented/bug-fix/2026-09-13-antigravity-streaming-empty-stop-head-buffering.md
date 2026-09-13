# Agent Note: Antigravity streaming empty stop suppression and error framing

Status: implemented

## Problem

When routing requests to Google Antigravity models (`gemini-3.8-flash-high`) in streaming mode (`stream: true`), Google's backend occasionally encounters prompt prefill overload or transient truncation (e.g. on prompts with large context > 100k tokens), immediately returning a single SSE frame with empty content parts (`parts: []`) and `finishReason: "STOP"`.

In the prior gateway architecture:
1. In non-streaming mode (`collect_antigravity_sse_to_json`), Ponyllm intercepted empty STOP frames and returned an Err, which triggered transparent gateway-side key rotation and retry.
2. In streaming mode (`antigravity_sse_to_openai_stream` and `antigravity_sse_to_anthropic_stream`), the gateway immediately flushed the HTTP 200 header and piped the upstream chunk as a valid `finish_reason: "stop"` chunk followed by `[DONE]`.
3. Downstream agent runtimes (such as DeepSeek Harness `@earendil-works/pi-ai` and Claude Code) saw a stream completed with `finish_reason: "stop"` but 0 text bytes and 0 tool calls, flagging an unrecoverable degenerate completion (`model returned a completed response with no content`).

## Decision

We implemented stream guard and empty stop suppression for Antigravity streaming requests:
1. In `antigravity_sse_to_openai_stream`:
   - Detected raw empty STOP chunks where `choices[0].finish_reason == Stop` and `delta` contains 0 text, 0 reasoning, and 0 tool calls.
   - When cumulative content bytes emitted so far are 0, suppressed the deceptive empty STOP chunk from being transmitted downstream.
   - If the stream finalizes with 0 content bytes and encountered an upstream empty STOP candidate, the gateway suppresses the deceptive normal finish and instead emits an explicit OpenAI-compatible SSE error event (`code: "EMPTY_RESPONSE", type: "server_error"`).
2. In `antigravity_sse_to_anthropic_stream`:
   - If the upstream stream finalizes with 0 content events, emitted an Anthropic `error` event (`type: "api_error"`) instead of empty synthetic termination.
3. Downstream clients (e.g. `pi-ai` in DSH, Claude Code, OpenAI SDK) receive an explicit error event and engage standard retry policies instead of crashing with degenerate completion assertions.
4. Added unit tests `test_antigravity_sse_to_openai_stream_zero_content_emits_error_frame` and `test_antigravity_sse_to_anthropic_stream_zero_content_emits_error_frame`.

## Alternatives considered

- **Pass-through with downstream client retries**: Rejected. Upstream transient failures are gateway concerns. Pushing empty STOP frames to clients causes client-side assertions and aborts agent multi-step loops unnecessarily.
- **Synthesizing fake content (e.g. single whitespace token)**: Rejected. This corrupts LLM generation, violates API contracts, and may mislead downstream tools expecting structured outputs or JSON.
- **Full stream buffering**: Rejected. Buffering the entire stream destroys the real-time interactivity and TTFT benefits of SSE streaming. Only buffering and inspecting terminal frames on zero content preserves streaming characteristics with zero visible latency overhead.

## Consequences

- Antigravity streams that encounter Google's transient empty STOP no longer push deceptive `finish_reason: "stop"` chunks to clients.
- Downstream clients correctly receive error notifications that trigger their standard error handling and retry mechanisms.
- All workspace tests pass cleanly.
- `.agents/skills/write-adr/verify-note.sh` exits 0.
