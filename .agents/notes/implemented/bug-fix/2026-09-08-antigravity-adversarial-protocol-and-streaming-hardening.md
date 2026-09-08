# Agent Note: Antigravity protocol fidelity and streaming error hardening

Status: implemented

## Problem

Adversarial audit of the Antigravity reverse proxy identified critical protocol fidelity and streaming reliability gaps:
1. Stream2NoStream false 200: `collect_antigravity_sse_to_json` blindly accumulated candidate parts and ignored upstream error frames (e.g. `{"error": {"code": 503, "message": "The model is overloaded"}}`), returning synthetic partial 200 responses to clients instead of propagating errors for transparent gateway failover.
2. Stream chunk stall: If the upstream stalled mid-stream without terminating or emitting keep-alive frames, the collector hung indefinitely without a per-chunk timeout guard.
3. Reasoning Effort Off budget leakage: Sending `ReasoningEffort::Off` without explicitly setting `thinkingBudget: 0` allowed Google's backend to apply default hidden thinking budgets and consume quota.
4. Thinking budget clamping constraint violation: When callers passed a small `max_tokens` (e.g. 500) alongside a thinking budget (e.g. 16384), simply setting `maxOutputTokens == thinkingBudget` violated Google Antigravity's strict contract `maxOutputTokens > thinkingBudget`, causing 400 Bad Request rejections.
5. Thought and content contamination: `antigravity_to_chat_response` and `antigravity_to_messages_response` mixed thinking parts (`thought: true`) with final output content, leaking internal thought tokens into user-facing content blocks.

## Decision

Landed all protocol fidelity and streaming hardening items in this change:

1. Intercept `val.get("error")` and `val.get("response").and_then(|r| r.get("error"))` in `collect_antigravity_sse_to_json`, returning descriptive `Err` to fail fast and classify as `UpstreamUnavailable` for retry/failover.
2. Introduce `collect_antigravity_sse_to_json_with_timeout` with a 15-second per-chunk timeout guard to abort stalled streams deterministically.
3. For non-gemini-3 models under `ReasoningEffort::Off`, explicitly emit `{"includeThoughts": false, "thinkingBudget": 0}` to ensure zero thinking token allocation.
4. Implement `clamp_max_output_for_thinking_budget` ensuring `maxOutputTokens` is elevated to `budget + 1024` whenever `cap <= budget`, satisfying Google's strict inequality constraint.
5. Decouple `thought: true` in `antigravity_chunk_to_chat_chunk`, `antigravity_to_chat_response`, and `antigravity_to_messages_response`: thinking content is cleanly directed into `reasoning_content` (OpenAI) or `{"type": "thinking"}` blocks (Anthropic), leaving regular content uncontaminated.
6. In `collect_antigravity_sse_to_json`, merge consecutive text parts sharing the same `thought` boolean, preventing fragmentation while keeping thinking and final answer separated.

## Alternatives considered

- **Return empty string or partial body on SSE error frames**: would avoid breaking downstream JSON parsing, but corrupts client state with fake 200s and blocks the gateway from triggering failover to healthy providers; rejected.
- **Rely solely on global HTTP client timeout instead of per-chunk SSE timeout**: long generations take several minutes, so a short global timeout aborts valid generation, while a long global timeout hangs on dropped TCP connections for minutes; per-chunk 15s timeout catches stalls cleanly without limiting total response duration. Chosen per-chunk timeout.
- **Set `maxOutputTokens = thinkingBudget + 1`**: technically satisfies the inequality, but leaves only 1 token for the actual answer, leading to immediate truncation; granting a 1024 token buffer allows the model to produce meaningful answers while honoring the requested thinking budget. Chosen `budget + 1024`.

## Consequences

- `cargo test --workspace` and `pnpm test` fully green across all 43 backend test suites and 13 frontend test suites.
- SSE error frames are reliably converted into failover-eligible errors.
- Thinking outputs are strictly separated into standard reasoning channels.
