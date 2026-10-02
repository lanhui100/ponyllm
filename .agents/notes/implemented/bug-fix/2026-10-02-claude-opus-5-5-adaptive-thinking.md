# Agent Note: claude-opus-5-5 adaptive thinking upstream protocol support

Status: implemented

## Problem
When calling `claude-opus-5-5` via Anthropic Messages upstream protocol, the upstream provider rejects requests with:
`400 Bad Request: claude-opus-5-5 requires adaptive thinking; omit thinking or use thinking.type=adaptive and output_config.effort`.
Currently, `ponyllm` hardcodes `thinking = {"type": "enabled", "effort": ...}` in `chat_anthropic`, `responses_anthropic`, and gateway route translation (`routes/messages.rs`, `routes/chat.rs`, `routes/responses.rs`). Because `claude-opus-5-5` strictly enforces the new Anthropic Adaptive Thinking protocol specification, requests fail with upstream exhausted 400 errors.

## Decision
For model `claude-opus-5-5` targeting Anthropic upstream protocol:
1. Extend `ThinkingConfig` and `MessageRequest` in `ponyllm-protocol` with support for `output_config`:
   - `output_config: Option<AnthropicOutputConfig>` where `AnthropicOutputConfig` contains `effort: Option<ReasoningEffort>`.
2. When formatting an Anthropic `MessageRequest` for `claude-opus-5-5`:
   - If thinking is active (`effective_thinking.is_active()`):
     - `thinking = Some(ThinkingConfig { r#type: "adaptive".to_string(), budget_tokens: None, effort: None })`
     - `output_config = Some(AnthropicOutputConfig { effort: Some(effective_thinking) })`
     - Do not serialize `effort` inside `thinking` when `type == "adaptive"`.
   - If thinking is off/disabled:
     - Omit `thinking` and `output_config` altogether (as accepted by upstream).
3. Preserve existing behavior (`type: "enabled"`) for all other models.

## Alternatives considered
1. **Omit thinking completely for claude-opus-5-5**:
   - Dropping `thinking` allows the request to pass without reasoning, but loses reasoning capability entirely even though the user configured `thinking_default = "high"`.
2. **Global change to type=adaptive for all Anthropic models**:
   - Claude 3.5 / 3.7 Sonnet and legacy Opus models only accept `type="enabled"` with optional `budget_tokens` and do not support `type="adaptive"`. Making it global would break older models.
   - Therefore, scoping to `claude-opus-5-5` (or future adaptive models) is strictly required.

## Consequences
- Requests to `claude-opus-5-5` through Anthropic upstream protocol correctly emit `{"thinking": {"type": "adaptive"}, "output_config": {"effort": "high"}}`.
- No regressions on other models using traditional thinking configurations.
