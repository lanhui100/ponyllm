# Agent Note: Faithful error propagation and retry-friendly contract for empty and blocked upstream streams

Status: implemented

## Problem

When AI coding agents (such as `deepseek-harness` or Claude Code) interact with `gemini-3.8-flash-high` through PonyLLM's Antigravity gateway, the agent turns intermittently and silently freeze mid-task without completing the objective.

Investigation revealed:
1. **Masking upstream anomalies with artificial content**: Commit `56b8f889` introduced a "Zero-Content Safeguard" that injected a fake whitespace chunk `{"content": " "}` whenever a stream stopped without emitting visible content.
2. **Premature turn termination**: Downstream agents like `deepseek-harness` (`dsh`) check assistant turn completion. When receiving a synthetic whitespace delta with `finish_reason: "stop"`:
   - It is parsed as a legitimate assistant message turn rather than an error or an empty response.
   - The agent treats the step as successful user-facing prose completion and terminates the current turn (`turn/end: completed`).
   - The user is left with an incomplete task and an agent that freezes until the user manually sends "continue".
3. **Suppression of downstream native retry mechanisms**: Harness frameworks like `dsh-llm-retry` explicitly recognize `EMPTY_RESPONSE` as an automatic retryable error code (with up to 5 exponential retries with jitter). Fabricating content prevents `EMPTY_RESPONSE` from triggering and breaks downstream self-healing.
4. **Silent upstream block & error frame handling**: If upstream Google Antigravity emits a `promptFeedback.blockReason` (safety filter), or candidates finish due to `SAFETY`, or an explicit error frame arrives in streaming mode, converting this to `finish_reason: "stop"` or silently injecting a space hides the true upstream fault. Upstream failures must not be silently swallowed or misrepresented as successful completions.

## Decision

We replace artificial content padding with faithful error and finish-reason propagation:

1. **Remove synthetic whitespace pad chunk from SSE streams**:
   - In `crates/ponyllm-server/src/streaming.rs` (`antigravity_sse_to_openai_stream`):
     - Remove the `pad_chunk` injection (`{"content": " "}`) in choice finish handling.
     - Remove the `pad_chunk` injection in the graceful terminal stream fallback.
   - If upstream produces a choice with `finish_reason: "stop"` and zero content blocks, let it emit the clean finish chunk. Clients configured to treat empty responses as retryable (`EMPTY_RESPONSE`) will automatically retry the request without manual human intervention.
2. **Faithful error and finish reason propagation**:
   - For `candidates[0].finishReason`:
     - Map `"SAFETY"` to OpenAI `FinishReason::ContentFilter`.
     - In non-streaming collector and stream translators, detect upstream `error` and `promptFeedback.blockReason` and propagate them as explicit errors or appropriate non-stop finish reasons rather than masking them as normal stops.
   - If upstream terminates via an error frame, emit the error frame cleanly or terminate the stream with a transport fault, ensuring downstream recognizes it as an error condition.

## Alternatives considered

- **Inject a visible warning message instead of whitespace (e.g. `[PonyLLM: Upstream produced empty response]`):**
  Rejected because injecting fake assistant text pollutes the conversation history and confuses the model on subsequent turns.
- **Synthesize a fake HTTP 500 error when stream completes with zero content:**
  Rejected because the stream has already emitted 200 OK headers. The proper OpenAI SSE contract is either an honest finish reason or an explicit `data: {"error": {...}}` frame. Downstream clients like `dsh` already handle empty completions via `EMPTY_RESPONSE` retry policies.

## Consequences

- Agents using `dsh` and similar frameworks will automatically trigger their retry loops on empty completions instead of mistaking them for turn completion.
- Upstream safety blocks and errors are visibly and faithfully reported instead of silently swallowed.
- Eliminates the bug where agents freeze mid-task and require manual "continue" prompts.
