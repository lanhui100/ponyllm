# Agent Note: Sanitize Empty Tool Response Name in Antigravity Translator

Status: implemented

## Problem

Under the Antigravity protocol, upstream Google Gemini / Vertex Protobuf schema strictly requires that all `function_response` parts contain a non-empty `name`:
```text
* GenerateContentRequest.contents[2].parts[0].function_response.name: Name cannot be empty.
```
In multi-turn chat completions, downstream tool responses (`role: "tool"`) or legacy function messages (`role: "function"`) can carry empty names if:
1. The client omits `name` and the previous assistant message's `tool_calls` had an unresolvable or empty tool name.
2. The legacy `FunctionMessage` carries an empty string in `name`.
3. OpenAI `ToolMessage` deserialization didn't support top-level `name`, causing it to drop client-supplied tool names.

When `functionResponse.name` becomes empty `""`, upstream returns HTTP 400 `INVALID_ARGUMENT`, causing the gateway to fail with `All candidate upstream providers exhausted`.

## Decision

1. Add optional `name` field to `ToolMessage` in `crates/ponyllm-protocol/src/openai/chat.rs`.
2. In `crates/ponyllm-protocol/src/translator/antigravity.rs`:
   - For `ChatMessage::Tool`, prioritize `m.name`, fallback to `tool_id_to_name`, and sanitize with a guaranteed non-empty fallback (`format!("tool_{}", sanitized_id)` or `"tool_result"`).
   - For `ChatMessage::Function`, ensure `func_name` falls back to `"tool_result"` if `m.name.trim().is_empty()`.
   - In Anthropic `ToolResult` block handling, guarantee `func_name` is never empty.

## Alternatives considered

- Reject request with 400 at gateway entrance: Rejected because downstream clients (e.g. OpenAI SDKs) frequently send valid tool responses without reproducing the function name, relying on the provider to resolve it from the call ID.
- Ignore the tool message: Rejected because downstream agents need the tool execution output in the conversation history for next-step reasoning.

## Consequences

- No more Protobuf schema validation failures on empty `function_response.name`.
- Downstream tool calls with missing or empty names are safely sanitized and forwarded to Google Gemini.
