# Agent Note: Antigravity end-to-end debug observability

Status: implemented

## Problem

When using the Antigravity upstream with models like `gemini-3.8-flash-high`, users encounter issues such as `model "gemini-3.8-flash-high" returned a completed response with no content`. Previously, the gateway and translator crates lacked structured debug logging across the Antigravity request lifecycle:
1. Envelope construction did not log translated `generationConfig` (e.g. `maxOutputTokens`, `thinkingConfig`, or whether `thinkingBudget` was omitted for Gemini 3 models).
2. Token lifecycle operations in `AntigravityTokenManager` did not log cache hits vs token refreshes, remaining lifetime, or 401 retry events.
3. Upstream HTTP request parameters and response headers were invisible in logs.
4. In `collect_antigravity_sse_to_json_with_timeout`, individual SSE frames arrived and merged without per-frame visibility into received content types (thought vs text vs function calls), finishReason, or token usage metadata. Crucially, when an upstream response finished with zero content bytes, no diagnostic warning was logged.
5. In streaming routes (`antigravity_sse_to_openai_stream` and `antigravity_sse_to_anthropic_stream`), downstream frame dispatch and completion conditions lacked visibility.
6. The CLI defaulted `tracing_subscriber` to `"ponyllm_server=info,tower_http=debug"`, silently discarding debug logs from `ponyllm_protocol` and `ponyllm_core`, and lacked a `--debug` flag.

## Decision

Implemented structured, leak-safe debug logging and CLI observability controls across all stages of the Antigravity pipeline:
1. CLI logging: Added `--debug` flag to `ponyllm serve` and `ponyllm web` subcommands. Configured default `EnvFilter` to include `ponyllm_server=debug,ponyllm_protocol=debug,ponyllm_core=debug` when debug mode is active, while preserving `RUST_LOG` environment precedence.
2. Translation and credentials: Added `tracing::debug!` logs in `chat_to_antigravity_request` and `messages_to_antigravity_request` capturing target model, request/session IDs, message counts, `maxOutputTokens`, and `thinkingConfig`. Added trace/debug/warn logging in `AntigravityTokenManager` for cache hits, refresh operations, and 401 recovery while maintaining full redaction of credential secrets.
3. Stream collecting and frame auditing: In `collect_antigravity_sse_to_json_with_timeout`, added per-frame tracking of chunk counts, thought bytes, text bytes, function calls, and finishReason. Added a prominent `tracing::warn!` diagnostic if the stream finishes with zero content bytes.
4. Response conversion: Added debug and zero-content warn logging to `antigravity_to_chat_response` and `antigravity_to_messages_response`.
5. Streaming dispatch: In `antigravity_sse_to_openai_stream` and `antigravity_sse_to_anthropic_stream`, added debug logging for stream initialization, terminal chunks, and finalization.

## Alternatives considered

- **Log raw HTTP request/response payloads in full**: Full JSON dumps could leak sensitive user conversation data and prompt contents into system log files; structured summaries with token counts, character lengths, and configuration keys provide exact diagnostics without privacy risks.
- **Rely solely on RUST_LOG environment variable without CLI flag**: Many users start the server with `ponyllm serve` directly without exporting environment variables; adding a `--debug` flag lowers the barrier to capturing diagnostics while preserving `RUST_LOG` precedence.
- **Place logging only in server route layer**: Route layer only sees high-level requests and final responses, completely missing internal FSM transitions, token clamping, and SSE frame-level thinking/text splits; comprehensive end-to-end logging requires instrumentation across protocol, core, and server.

## Consequences

- `cargo test --workspace` passes cleanly across all crates.
- Users can run `ponyllm serve --debug` (or use `RUST_LOG`) to pinpoint the root cause of zero-content responses with exact frame-by-frame and token-level details.
- No credential secrets or prompt contents are leaked in logs.
