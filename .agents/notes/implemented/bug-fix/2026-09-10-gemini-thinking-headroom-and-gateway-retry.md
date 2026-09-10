# Agent Note: Headroom expansion for Gemini thinking budgets and transparent gateway retry

Status: implemented

## Problem

When AI coding agents (such as Claude Code, Codex, Cursor, and deepseek-harness) invoke `gemini-3.8-flash-high` or reasoning models through PonyLLM, requests can intermittently yield zero content, either triggering client-side assertions or breaking agent task execution:

1. **Thinking token budget exhaustion**:
   - `gemini-3.8-flash-high` uses an implicit thinking budget of up to 16,384 tokens (or 8,192 / 2,048 depending on tier).
   - In Gemini's protocol, `generationConfig.maxOutputTokens` caps the **sum of thinking tokens + visible content/tool tokens**.
   - When client requests specify a modest `max_tokens` (e.g. 1,024 to 4,096 tokens), Gemini exhausts all output tokens inside the thinking process and terminates immediately with `finishReason: "MAX_TOKENS"`.
   - The response contains only thought parts and zero visible text or tool calls. Previously, `clamp_max_output_for_thinking_budget` only raised `maxOutputTokens` to `budget + 1024` if `cap <= budget`. A 1,024-token margin is inadequate for complex coding tasks where agents invoke tools with extensive arguments, causing frequent truncation.
2. **Upstream transient empty STOP completions**:
   - Under heavy context or specific prompt patterns, Google Antigravity intermittently completes with `STOP` while emitting 0 text parts and 0 tool calls.
   - Claude Code, Codex, and OpenAI SDKs fail with strict validation errors upon receiving an empty completion.
   - Injecting fake whitespace (`{"content": " "}`) disguises failures as successful answers, which prematurely ends agent turns.

## Decision

We implement a dual-layer safeguard in the Antigravity protocol translator and gateway execution path:

1. **Sufficient headroom expansion (`budget + requested_max`)**:
   - In `clamp_max_output_for_thinking_budget` (`crates/ponyllm-protocol/src/translator/antigravity.rs`), when a thinking budget is present or inferred for Gemini models:
     - The output cap `maxOutputTokens` is guaranteed to be at least `budget + requested_cap`, ensuring that thinking tokens never crowd out or cannibalize the tokens allocated for visible output and tool calls.
     - We enforce a minimum visible output margin of at least 8,192 tokens (or up to Gemini's 65,536 limit) when high reasoning effort is requested.
2. **Accurate `finish_reason` translation**:
   - Ensure `MAX_TOKENS` from upstream is cleanly translated to `length` (OpenAI) and `max_tokens` (Anthropic), alerting clients that context/output was truncated rather than completed normally.
   - Map `SAFETY` to `content_filter` (OpenAI) and `stop_sequence` (Anthropic).
3. **Transparent gateway-side retry on empty STOP**:
   - In non-streaming collector and retryable stream paths, if an upstream candidate terminates with `finishReason: "STOP"` but contains zero visible content (no text, no tool calls) and no error frame, treat the attempt as a transient failure and allow the gateway failover/retry loop to re-dispatch the request instead of surfacing a broken empty response to vulnerable downstream clients.

## Alternatives considered

- **Force thinking budget to 0**: Rejected because it degrades the reasoning and coding quality of `gemini-3.8-flash-high`.
- **Rely solely on client-side retry**: Rejected because external tools like Claude Code and various IDE extensions crash fatally on empty responses before their retry loops can take effect.
- **Synthesize whitespace pad**: Rejected because it tricks agent state machines into believing the turn completed successfully.

## Consequences

- Completely eliminates token-exhaustion truncations during thinking on `gemini-3.8-flash-high`.
- Downstream tools (Claude Code, Codex, dsh) receive complete responses with full tool-calling arguments.
- Transient upstream empty responses are healed transparently at the gateway layer without user friction.
