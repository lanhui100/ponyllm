# Agent Note: Wire-Safe Tool and Function Name Sanitization

Status: implemented

## Problem

When models (e.g., `gemini-3.8-flash` on Antigravity) hallucinate tool calls with invalid identifiers such as `git_diff:bash` or `git_log_show:bash`, the agent harness (such as DeepSeek Harness / DSH) stores these tool calls in session conversation history.

When the session subsequently routes requests to strict upstreams (such as `opencode-zen` serving `muse-spark-1.3-contributor-free` using the Responses protocol), the translator emits `FunctionCall { name }` directly into the upstream body. Strict upstreams validate `name` against `^[a-zA-Z0-9_.-]+$` and reject the request with HTTP 400 Bad Request (`"message": "\`name\` must match ^[a-zA-Z0-9_.-]+$"`, `param: "name"`).

Because the invalid tool call remains in the session's historical turns, every subsequent turn for that session that routes to the strict upstream repeatedly fails with HTTP 400, effectively poisoning the session across all keys.

## Decision

1. In `ponyllm-protocol`, introduce a centralized, deterministic sanitizer `sanitize_wire_tool_name(raw: &str) -> String`.
   - Any character not matching `[a-zA-Z0-9_.-]` is replaced with `_`.
   - Empty or whitespace-only names are replaced with a safe fallback identifier (`tool_call`).
   - Names exceeding 64 characters (OpenAI / Responses specification limit) are truncated to 56 characters and suffixed with an 8-character hex hash of the original name to prevent collisions.
   - Idempotent: already valid names are preserved untouched without allocation where possible.

2. Apply this sanitization across protocol conversion boundaries:
   - `chat_to_responses_request`: replayed historical assistant `tool_calls[*].function.name` (where `ResponseInputItem::FunctionCall` is emitted) and declared `tools[*].name`.
   - `chat_to_anthropic_request`: replayed historical `tool_use.name` and declared `tools[*].name`.
   - `responses_to_anthropic_request`: replayed historical `tool_use.name` and declared `tools[*].name`.
   - `chat_to_antigravity_request`: declared function names and replayed `functionCall.name`.

3. Replay safety: tool call execution pairings rely on `tool_call_id` / `call_id` across all protocols (`tool_call_id` in Chat, `call_id` in Responses, `tool_use_id` in Anthropic), not on `name`. Therefore, sanitizing `name` in historical tool call records does not break tool response pairing, while preventing upstream 400 rejection.

## Alternatives considered

- **Client/Harness-only fix**: Fix solely in DeepSeek Harness (discard unknown tool calls from history).
  - *Rejected*: PonyLLM acts as a multi-protocol gateway serving multiple clients and third-party tools. Upstream protocol constraints should be defended at the gateway boundary to protect against any client or weak model hallucination.

- **Drop invalid historical tool calls at gateway**: Completely remove assistant tool calls with invalid names and their corresponding tool result messages.
  - *Rejected*: Alters conversation history structure, risks leaving orphaned user/assistant message sequences or breaking prompt alignment expected by the caller.

- **Bidirectional stateful name mapping**: Track a per-request map from sanitized names back to original names during response generation.
  - *Rejected*: Unnecessary for historical tool calls (which were already executed or failed). For newly declared tools, legitimate tool names across all major APIs (OpenAI, Anthropic, Gemini) already adhere to `[a-zA-Z0-9_.-]`. Keeping stateful mapping adds significant complexity to streaming response translators with negligible benefit.
