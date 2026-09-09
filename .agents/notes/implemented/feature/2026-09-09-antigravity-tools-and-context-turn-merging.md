# Agent Note: Antigravity tool calling support and context turn merging

Status: implemented

## Problem

When AI coding tools (such as Claude Code, Cursor, Cline, and Roo Code) invoked `gemini-3.8-flash-high` through the Antigravity reverse proxy, requests continuously failed with `model "gemini-3.8-flash-high" returned a completed response with no content`:
1. **Dropped tools declarations**: `chat_to_antigravity_request` and `messages_to_antigravity_request` dropped client-provided `tools` definitions entirely, failing to map them to Gemini `functionDeclarations`.
2. **Broken multi-turn tool calling history**: Prior assistant messages containing `tool_calls` without text content were converted into empty `parts: [{"text": ""}]` instead of native `functionCall` parts. Similarly, `ChatMessage::Tool` results were translated into plain text `role: "user"` rather than Gemini `functionResponse` parts, breaking the function-call state machine upstream.
3. **Non-alternating adjacent roles**: Clients frequently send consecutive `user` turns (e.g. user prompt followed by `<system-reminder>` guidance), violating Gemini API's strict alternation between `user` and `model` turns and leading to silent immediate `STOP` completions with 0 bytes.
4. **Missing Anthropic tool_use mapping**: In `antigravity_to_messages_response`, upstream `functionCall` was not mapped to Anthropic `tool_use` blocks.

## Decision

We implemented native bidirectional tool calling and turn normalization for the Antigravity translator:

1. **Tool Declarations Mapping**:
   - Mapped OpenAI `ToolDefinition` and Anthropic `AnthropicTool` to Gemini `tools: [{"functionDeclarations": [...]}]`.
   - Maintained the required Antigravity CLI fingerprint `toolConfig: {"functionCallingConfig": {"mode": "VALIDATED"}}`.
2. **Context Turn Merging (`push_or_merge_turn`)**:
   - Implemented automatic adjacent turn consolidation: consecutive messages with the same role (`user` or `model`) are merged into a single turn with multiple parts.
3. **Multi-Turn Tool Call & Result Fidelity**:
   - Reconstructed Assistant messages with `tool_calls` into native Gemini `functionCall` parts without generating empty `text: ""` parts.
   - Reconstructed Tool results into Gemini `functionResponse` parts mapped by `tool_call_id`.
   - Injected `thoughtSignature: "skip_thought_signature_validator"` at the Part level (parallel to `functionCall`) to satisfy Gemini 3.x's requirement for thought signatures in functionCall parts across stateless client turns without throwing protobuf schema violations.
   - Extended `antigravity_to_messages_response` to decode upstream `functionCall` into Anthropic `tool_use` blocks with `stop_reason: "tool_use"`.

## Alternatives considered

- **Flatten all tool calls and results to plain text prompts**: Avoids Gemini function calling schema, but breaks schema-guided structured outputs, loses tool parameter validation, and prevents tools from functioning reliably in Agent workflows. Rejected.
- **Reject non-alternating user messages with 400 Bad Request**: Forces client compliance, but real-world AI IDEs and CLI agents routinely emit consecutive user messages with system reminders. Merging adjacent same-role turns into multi-part turns is fully compliant with Gemini's API specification and seamlessly preserves client compatibility. Chosen.
- **Nest `thought_signature` inside `functionCall` object**: Protobuf schema `FunctionCall` does not define `thought_signature`, causing 400 `INVALID_ARGUMENT: Cannot find field`. Google requires the signature on the enclosing `Part` object (`thoughtSignature`). Placed at `part` level. Chosen.

## Consequences

- Full multi-turn Agent tool calling workflows function cleanly over both Chat Completions and Anthropic Messages endpoints without 400 thought_signature validation errors.
- Consecutive user messages and system reminders are safely unified into valid multi-part turns.
- All 40 unit and integration tests in `ponyllm-protocol` and full workspace tests pass.
