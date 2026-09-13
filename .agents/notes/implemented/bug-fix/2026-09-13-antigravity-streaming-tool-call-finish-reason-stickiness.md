# Agent Note: Antigravity streaming tool call finish reason stickiness fix

Status: implemented

## Problem

When using `gemini-3.8-flash-high` through Ponyllm's Antigravity upstream, downstream AI coding agents (such as DeepSeek Harness, pi-ai, or Claude Code) frequently fail with:
`model "gemini-3.8-flash-high" returned a completed response with no content`.

Live telemetry audit revealed the root cause:
1. Gemini's Antigravity backend emits SSE responses in multi-frame chunks. In frame 1, it outputs a tool call (`functionCall`) with no finish reason (`finishReason == null`).
2. In frame 2, it outputs an empty candidate (`parts: []`) with `finishReason: "STOP"`.
3. Because `antigravity_chunk_to_chat_chunk` determines `finish_reason` statelessly per-chunk (`if has_tools { FinishReason::ToolCalls } else { FinishReason::Stop }`), frame 2 has empty parts (`has_tools == false`), so it maps `finishReason: "STOP"` to `FinishReason::Stop` instead of `FinishReason::ToolCalls`.
4. Downstream clients (e.g. pi-ai) interpret `finish_reason: "stop"` as the conclusion of a text-only turn. Because `text_bytes == 0` (the model was attempting to call a tool), pi-ai flags the completion as an empty degenerate response (`EMPTY_RESPONSE`) and aborts with `"returned a completed response with no content"`.

## Decision

We made the `tool_calls` finish reason sticky across the streaming session in `crates/ponyllm-server/src/streaming.rs`:
1. In `antigravity_sse_to_openai_stream`, tracked whether tool calls were ever emitted during the stream (`total_tool_calls > 0`).
2. When any chunk carries `finish_reason: Some(FinishReason::Stop)`, if tool calls were emitted in this or any previous frame, override the chunk's finish reason to `FinishReason::ToolCalls`.
3. If synthetic termination is needed at EOF, preserve `FinishReason::ToolCalls` when tool calls were emitted.
4. In `antigravity_sse_to_anthropic_stream`, tracked tool call emission and applied the same sticky correction before passing the chunk to the Anthropic FSM, ensuring it cleanly maps to `tool_use`.
5. Added unit test `test_antigravity_sse_to_openai_stream_multiframe_tool_calls_stickiness` verifying that multi-frame tool calls with an empty terminal STOP frame maintain `finish_reason: "tool_calls"`.

## Alternatives considered

- **Modify `antigravity_chunk_to_chat_chunk` in `ponyllm-protocol` directly**: That function is pure and stateless; it does not retain historical state across SSE stream frames. Keeping state tracking inside the streaming orchestrator in `ponyllm-server` preserves protocol purity and aligns with the existing `has_emitted_chunks` architecture.
- **Client-side workaround**: Attempting to alter downstream client assertions would not fix other OpenAI-compatible consumers who also expect `finish_reason: "tool_calls"` when tools are executed.

## Consequences

- Multi-frame Antigravity streams emitting `functionCall` in frame 1 and `finishReason: "STOP"` in frame 2 reliably output `finish_reason: "tool_calls"`.
- Downstream clients (pi-ai / DSH) no longer trigger `EMPTY_RESPONSE` / `"returned a completed response with no content"` on tool call turns.
- All workspace tests pass cleanly.
- `.agents/skills/write-adr/verify-note.sh` exits 0.
