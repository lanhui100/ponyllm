# Agent Note: Antigravity streaming zero-content and upstream error observability hardening

Status: implemented

## Problem

When downstream AI coding agents (such as DeepSeek Harness, Claude Code, or Roo/Cline) use `gemini-3.8-flash-high` through Ponyllm's Antigravity upstream with `stream: true`, users occasionally encounter the error:
`model "gemini-3.8-flash-high" returned a completed response with no content`.
Downstream agent loops retry this up to 5 times before failing completely.

Currently, the streaming proxy (`antigravity_sse_to_openai_stream` and `antigravity_sse_to_anthropic_stream`) in `crates/ponyllm-server/src/streaming.rs` suffers from several critical observability blindspots:
1. When upstream returns an `error` frame or a `promptFeedback.blockReason` (such as content moderation or quota warning), the streaming converter simply fails `serde_json` field extraction or choice matching and silently drops the frame.
2. The streaming translator faithfully passes through empty `finishReason: "STOP"` when upstream terminates with zero parts, but unlike the non-streaming collector (`collect_antigravity_sse_to_json`), it emitted no diagnostic warning when a stream ended with zero content bytes and zero tool calls.
3. Because Ponyllm's default log level suppresses debug traces from `ponyllm_server`, operators could not see why downstream received an empty completion or what finishReason Google's backend returned.

## Decision

We hardened the Antigravity streaming translation pipeline with structured, leak-safe diagnostics and explicit error auditing:
1. In `antigravity_sse_to_openai_stream` and `antigravity_sse_to_anthropic_stream`, tracked payload volume across frames: `total_frames`, `text_bytes`, `thought_bytes`, `tool_call_count`, and the latest `finish_reason`.
2. Explicitly inspect incoming SSE JSON objects for upstream `error` and `promptFeedback` frames, logging a `tracing::error!` with the upstream error code/message before forwarding or completing.
3. When the stream completes (either on receiving a terminal choice or on EOF), if `text_bytes == 0 && tool_call_count == 0`, emit a prominent `tracing::warn!` documenting the anomaly (model, response_id, frame count, thought bytes, finish reason) so that zero-content empty completions are immediately visible in standard `info`-level server logs.
4. Added regression tests verifying upstream error frame parsing without panic or corrupt choices. Ensure no prompt contents, sensitive user data, or API keys are leaked into logs.

## Alternatives considered

- **Synthesize whitespace pad tokens (revert commit 4ac64944)**: Injecting fake `" "` content masks the upstream failure and causes coding agents to parse empty tools or corrupt ASTs. Honest pass-through coupled with explicit diagnostic warnings is the correct architecture.
- **Rely solely on verbose `--debug` flags**: Users running in production with default settings (`info` level) would continue to see no log output when empty responses occur. A `tracing::warn!` for a genuine anomaly (0 text bytes, 0 tool calls) is appropriate for default logging.

## Consequences

- Both `antigravity_sse_to_openai_stream` and `antigravity_sse_to_anthropic_stream` detect upstream error frames and log `tracing::error!`.
- When an Antigravity stream terminates with zero text and zero tool calls, a prominent `tracing::warn!` is emitted.
- All workspace tests (`cargo test --workspace`) pass without regressions.
- `.agents/skills/write-adr/verify-note.sh` exits 0.
