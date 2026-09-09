# Agent Note: Thinking-aware output token safeguard and capacity upgrade

Status: implemented

## Problem

Client applications (such as Claude Code, Cursor, Codex, and Vercel AI SDK) regularly default to modest `max_tokens` / `max_completion_tokens` parameters (e.g., 2048 or 4096 tokens). When calling reasoning/thinking models (e.g., Gemini 3.8 Flash High, Claude 3.7 Sonnet Thinking, OpenAI o-series), two major issues led to failures with empty content:
1. **Thinking token exhaustion**: Models consume the allocated token budget generating reasoning/thinking thoughts first. When the client sends `max_tokens: 2048` or `4096`, the entire quota is exhausted before generating final response text, triggering `finish_reason: "length"` with zero visible content (`model "gemini-3.8-flash-high" returned a completed response with no content`).
2. **Tool call translation drop in Antigravity translator**: In `antigravity_chunk_to_chat_chunk` and non-streaming `antigravity_to_chat_response`, `functionCall` parts were not mapped to `tool_calls` / `FinishReason::ToolCalls`, causing empty text content and swallowed tool invocations.
3. **Conservative output capacity limits**: 1M context models (such as `gemini-3.8-flash-high`) had physical `max_output` capped at 16K in default configuration, artificially restricting output capabilities well below the upstream model's 32K/64K limits.

## Decision

We implemented a comprehensive, model-agnostic, and protocol-wide safeguard:

1. **Thinking-Aware Output Safeguard (`apply_thinking_output_safeguard`)**:
   - Introduced safe floor token budgets based on detected `ReasoningEffort`:
     - `High`: Minimum 16,384 tokens.
     - `Medium`: Minimum 8,192 tokens.
     - `Low`: Minimum 4,096 tokens.
     - `None`: Retains client's original `max_tokens`.
   - Capped deterministically by the model's configured physical `max_output` limit.
   - Unified across all protocol entry points:
     - Chat Completions (`/v1/chat/completions`): updates `max_tokens` and `max_completion_tokens`.
     - Anthropic Messages (`/v1/messages`): updates `max_tokens`.
     - OpenAI Responses (`/v1/responses`): updates `max_output_tokens`.
2. **Antigravity Tool Call and Clamping Fidelity**:
   - Enhanced `antigravity_chunk_to_chat_chunk` and `antigravity_to_chat_response` to decode `functionCall` parts into `tool_calls` and synthesize `FinishReason::ToolCalls`.
   - Updated `clamp_max_output_for_thinking_budget` to recognize Gemini 3 high/low implicit reasoning effort, elevating output budget to satisfy Google's strict `maxOutputTokens > thinkingBudget` constraint with at least 1024 tokens headroom.
3. **Configuration & Capacity Upgrade**:
   - Elevated physical `max_output` for 1M models (`gemini-3.8-flash-high`) from 16,384 to 32,768 in `/home/dm/pproxy/ponyllm.toml`.

## Alternatives considered

- **Client-only fix (require callers to set `max_tokens: 32768`)**: Rejected because third-party clients and tools (e.g. Claude Code, CLI agents, IDE extensions) hardcode or constrain output token limits, and requiring end-user reconfiguration across disparate tools is fragile and impractical.
- **Unconditionally elevate `max_tokens` for all models to 32K**: Rejected because non-reasoning models (or models with small context windows) would receive unexpected parameter overrides or exceed upstream provider limits. Safeguard is strictly gated on reasoning models and thinking effort levels.
- **Set `maxOutputTokens = thinkingBudget + 1` in Antigravity translator**: Rejected because a 1-token headroom causes immediate truncation upon generating the first character of the answer. A minimum headroom of 1024 tokens allows meaningful answers.

## Consequences

- Completely eliminates empty-response retry storms (`completed response with no content`) caused by token exhaustion under thinking models.
- Tool calling via Antigravity reverse proxy correctly propagates tool invocations across streaming and non-streaming responses.
- 100% test coverage and regression suites in `thinking_output_safeguard_tests.rs` and `translator_tests.rs`; `cargo test --workspace` fully passes.
