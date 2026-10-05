# Agent Note: Downstream SSE Heartbeat to Prevent Client Idle Timeout

Status: implemented

## Problem

When models with long reasoning phases (such as Gemini 3.8 Flash High under Antigravity, or other reasoning models) enter deep thought or experience upstream scheduling pauses, there can be long periods (e.g. tens of seconds to minutes) where the upstream produces no text or token chunks.

Downstream consumers (such as DeepSeek Harness `@deepseek-ai/dsh-llm-pi-ai` with `streamIdleTimeoutMs: 300000`, as well as standard reverse proxies like Nginx, Cloudflare, Traefik) have idle read timeouts. If the gateway emits no bytes downstream during long thinking or scheduling delays, downstream clients abort the connection with `stream idle timeout` (e.g. `pi-ai stream idle timeout after 300000ms`).

## Decision

Introduce an SSE heartbeat mechanism (`sse_heartbeat_guard`) on streaming responses emitted to clients:
1. Wrap outbound SSE byte streams with a periodic heartbeat interval (defaulting to 15 seconds of silence).
2. If no data chunk is emitted to the client within the heartbeat interval, inject an SSE comment line (`: ping\n\n` or `: keepalive\n\n`) into the downstream byte stream.
3. According to W3C SSE specifications and standard client decoders (including OpenAI SDK `SSEDecoder`), leading-colon comment lines are discarded by JSON parsers without disturbing application state, but they actively transmit wire bytes over the TCP/HTTP connection to reset downstream idle watchdogs and keep intermediate proxies alive.
4. Integrate this guard directly in the gateway streaming pipeline so that all downstream protocols (OpenAI completions, Anthropic messages, OpenAI responses) benefit transparently.

## Alternatives considered

1. **Only adjust downstream client timeout configurations**:
   - For example, increasing `streamIdleTimeoutMs` in DSH `cordis.patch.yml`.
   - *Rejected*: This does not solve timeouts caused by intermediate proxies (Traefik, Nginx, Cloudflare) which may have fixed 60s read timeouts. It also requires reconfiguring every downstream client instead of solving it once at the gateway.

2. **Send synthetic empty delta chunks instead of SSE comments**:
   - For example, sending `data: {"choices":[{"delta":{}}]}\n\n`.
   - *Rejected*: Incompatible with strict client schemas and state machines (which may count chunks, expect tokens, or fail schema validation on empty frames). SSE comments (`: ping\n\n`) are the official W3C SSE mechanism specifically designed for connection keepalive.

## Consequences

- Clients and proxy watchdogs are periodically refreshed with wire bytes during long reasoning pauses.
- SSE comment lines are safely ignored by OpenAI SDK and other W3C-compliant SSE parsers.
- No regression on stream completion, token accounting, or telemetry metrics (telemetry already ignores comment lines via `estimate_tokens_from_sse_bytes` and `parse_event_lines`).
